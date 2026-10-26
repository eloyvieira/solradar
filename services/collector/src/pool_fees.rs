use anyhow::{bail, Context, Result};
use base64::{
    engine::general_purpose::STANDARD,
    Engine as _,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{MySqlPool, Row};
use std::time::Duration;
use tokio::time::{interval, sleep, MissedTickBehavior};

struct PoolFee {
    pct: f64,
    fee_type: &'static str,
}

fn anchor_check(data: &[u8], name: &str) -> Result<()> {
    let hash = Sha256::digest(
        format!("account:{name}").as_bytes()
    );
    if data.get(..8) != Some(&hash[..8]) {
        bail!("Invalid discriminator: {name}");
    }
    Ok(())
}

fn u16_at(data: &[u8], offset: usize) -> Result<u16> {
    let bytes: [u8; 2] = data
        .get(offset..offset + 2)
        .context("u16 out of bounds")?
        .try_into()?;

    Ok(u16::from_le_bytes(bytes))
}

fn u32_at(data: &[u8], offset: usize) -> Result<u32> {
    let bytes: [u8; 4] = data
        .get(offset..offset + 4)
        .context("u32 out of bounds")?
        .try_into()?;

    Ok(u32::from_le_bytes(bytes))
}

fn pubkey_at(data: &[u8], offset: usize) -> Result<String> {
    let bytes = data
        .get(offset..offset + 32)
        .context("Pubkey out of bounds")?;

    Ok(bs58::encode(bytes).into_string())
}

async fn rpc_account(
    client: &reqwest::Client,
    rpc_url: &str,
    address: &str,
) -> Result<Vec<u8>> {
    let key = bs58::decode(address).into_vec()?;

    if key.len() != 32 {
        bail!("Invalid Solana address: {address}");
    }

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getAccountInfo",
        "params": [
            address,
            {
                "encoding": "base64",
                "commitment": "confirmed"
            }
        ]
    });

    let response = client
        .post(rpc_url)
        .json(&request)
        .send()
        .await?
        .error_for_status()?;

    let body: Value = response.json().await?;

    if let Some(error) = body.get("error") {
        bail!("RPC error: {error}");
    }

    let encoded = body["result"]["value"]["data"][0]
        .as_str()
        .context("Account not found")?;

    Ok(STANDARD.decode(encoded)?)
}

async fn orca_fee(
    client: &reqwest::Client,
    rpc_url: &str,
    address: &str,
) -> Result<PoolFee> {
    let data = rpc_account(
        client, rpc_url, address
    ).await?;

    anchor_check(&data, "Whirlpool")?;

    if data.len() != 653 {
        bail!("Orca layout not supported");
    }

    let rate = u16_at(&data, 45)?;

    if rate >= 1_000_000 {
        bail!("Invalid Orca fee");
    }

    Ok(PoolFee {
        pct: rate as f64 / 10_000.0,
        fee_type: "BASE",
    })
}

async fn raydium_clmm_fee(
    client: &reqwest::Client,
    rpc_url: &str,
    address: &str,
) -> Result<PoolFee> {
    let pool = rpc_account(
        client, rpc_url, address
    ).await?;

    anchor_check(&pool, "PoolState")?;

    let config_address = pubkey_at(&pool, 9)?;

    let config = rpc_account(
        client,
        rpc_url,
        &config_address,
    ).await?;

    anchor_check(&config, "AmmConfig")?;

    let rate = u32_at(&config, 47)?;

    if rate >= 1_000_000 {
        bail!("Invalid Raydium fee");
    }

    Ok(PoolFee {
        pct: rate as f64 / 10_000.0,
        fee_type: "BASE",
    })
}

async fn get_pool_fee(
    client: &reqwest::Client,
    rpc_url: &str,
    dex_id: &str,
    address: &str,
) -> Result<Option<PoolFee>> {
    match dex_id {
        "orca" => Ok(Some(
            orca_fee(client, rpc_url, address).await?
        )),

        "raydium-clmm" => Ok(Some(
            raydium_clmm_fee(
                client, rpc_url, address
            ).await?
        )),

        "pumpswap"
        | "meteora"
        | "meteora-damm-v2"
        | "manifest"
        | "zerofi"
        | "humidifi"
        | "raydium"
        | "jup-lend-amm"
        | "pancakeswap-v3-solana" => Ok(None),

        _ => Ok(None),
    }
}

pub async fn update_pool_fees(
    db: &MySqlPool,
    client: &reqwest::Client,
    rpc_url: &str,
    spacing_ms: u64,
) -> Result<()> {
    let rows = sqlx::query(
        r#"
        SELECT pool_address, dex_id
        FROM solana_pools
        ORDER BY dex_id, pool_address
        "#
    )
    .fetch_all(db)
    .await?;

    let mut updated = 0_u64;
    let mut failed = 0_u64;
    let mut unsupported = 0_u64;

    for row in rows {
        let address: String = row.try_get("pool_address")?;
        let dex_id: String = row.try_get("dex_id")?;
        let supported = matches!(
            dex_id.as_str(),
            "orca" | "raydium-clmm"
        );
 
        if !supported {
            unsupported += 1;
            continue;
        }

        match get_pool_fee(
            client,
            rpc_url,
            &dex_id,
            &address,
        ).await {
            Ok(Some(fee)) => {
                let result = sqlx::query(
                    r#"
                    UPDATE solana_pools
                    SET
                        fee_pct = ?,
                        fee_type = ?,
                        fee_updated_at = UTC_TIMESTAMP(6)
                    WHERE pool_address = ?
                    "#
                )
                .bind(format!("{:.8}", fee.pct))
                .bind(fee.fee_type)
                .bind(&address)
                .execute(db)
                .await;

                match result {
                    Ok(_) => {
                        updated += 1;

                        println!(
                            "POOL FEE UPDATED \
                             dex={} pool={} fee={:.6}%",
                            dex_id, address, fee.pct
                        );
                    }

                    Err(error) => {
                        failed += 1;

                        eprintln!(
                            "POOL FEE DB ERROR \
                             dex={} pool={} error={:#}",
                            dex_id, address, error
                        );
                    }
                }
            }

            Ok(None) => {
                unsupported += 1;
            }

            Err(error) => {
                failed += 1;

                eprintln!(
                    "POOL FEE ERROR \
                     dex={} pool={} error={:#}",
                    dex_id, address, error
                );
            }
        }

        sleep(
            Duration::from_millis(spacing_ms)
        ).await;
    }

    println!(
        "POOL FEES COMPLETE \
         updated={} failed={} unsupported={}",
        updated, failed, unsupported
    );

    Ok(())
}

pub async fn run(
    db: MySqlPool,
    client: reqwest::Client,
    rpc_url: String,
    interval_seconds: u64,
    spacing_ms: u64,
) {
    let mut timer = interval(
        Duration::from_secs(
            interval_seconds.max(60)
        )
    );

    timer.set_missed_tick_behavior(
        MissedTickBehavior::Skip
    );

    loop {
        timer.tick().await;

        if let Err(error) = update_pool_fees(
            &db,
            &client,
            &rpc_url,
            spacing_ms,
        ).await {
            eprintln!(
                "POOL FEES UPDATE ERROR: {error:#}"
            );
        }
    }
}

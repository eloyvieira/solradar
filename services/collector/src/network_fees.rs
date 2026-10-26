use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sqlx::MySqlPool;
use std::time::Duration;
use sqlx::Row;
use tokio::time::{interval, MissedTickBehavior};

pub async fn get_recent_priority_fees(
    client: &reqwest::Client,
    rpc_url: &str,
    accounts: &[String],
) -> Result<Vec<u64>> {

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getRecentPrioritizationFees",
        "params": [accounts]
    });

    let delays = [2_u64, 5, 10];

    for attempt in 0..=delays.len() {

        let response = client.post(rpc_url).json(&request).send().await?;
        let status = response.status();

        if status.as_u16() == 429 {
            if attempt == delays.len() {
                bail!("Solana RPC HTTP 429 após 4 tentativas");
            }

            let wait = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(delays[attempt]);

            eprintln!(
                "SOLANA RPC RATE LIMIT: aguardando {}s",
                wait
            );

            tokio::time::sleep( Duration::from_secs(wait) ).await;
            continue;
        }

        let response = response.error_for_status()?;
        let body: Value = response.json().await?;

        if let Some(error) = body.get("error") {
            bail!("Solana RPC: {error}");
        }

        let entries = body["result"].as_array().context("Invalid RPC response")?;
        let mut samples: Vec<(u64, u64)> = entries
            .iter()
            .filter_map(|entry| {
                Some((
                    entry["slot"].as_u64()?,
                    entry["prioritizationFee"].as_u64()?,
                ))
            })
            .collect();

        samples.sort_unstable_by(|a, b| b.0.cmp(&a.0));

        return Ok(
            samples
                .into_iter()
                .take(30)
                .map(|(_, fee)| fee)
                .collect()
        );
    }
    unreachable!()
}

fn percentile_75(mut fees: Vec<u64>) -> Result<u64> {
    if fees.is_empty() {
        bail!("No fee sample received");
    }
    fees.sort_unstable();
    let index = (fees.len() * 3).div_ceil(4) - 1;
    Ok(fees[index])
}

pub async fn update_network_fees(
    db: &MySqlPool,
    client: &reqwest::Client,
    rpc_url: &str,
) -> Result<()> {

    let rows = sqlx::query(
        r#"
        SELECT pool_address
        FROM solana_pools
        WHERE last_seen >= DATE_SUB(
            UTC_TIMESTAMP(), INTERVAL 5 MINUTE
        )
        ORDER BY last_seen DESC
        "#
    )
    .fetch_all(db)
    .await?;

	let mut updated = 0_u64;
    let mut failed = 0_u64;

    for row in rows {
        let pool_address: String = row.try_get("pool_address")?;
        let accounts = vec![pool_address.clone()];
        let fees = match get_recent_priority_fees(
            client,
            rpc_url,
            &accounts,
        ).await {
            Ok(fees) => fees,
            Err(error) => {
                failed += 1;
                eprintln!(
                    "NETWORK FEE RPC ERROR pool={} error={:#}",
                    pool_address,
                    error
                );
                continue;
            }
        };

        let priority_fee = match percentile_75(fees) {
            Ok(fee) => fee,
            Err(error) => {
                failed += 1;
                eprintln!(
                    "NETWORK FEE EMPTY pool={} error={:#}",
                    pool_address,
                    error
                );
                continue;
            }
        };

        let estimated_fee = 5_000_u64.saturating_add(priority_fee);

        sqlx::query(
            r#"
            UPDATE solana_pools
            SET
                network_fee_lamports = ?,
                network_fee_updated_at = UTC_TIMESTAMP(6)
            WHERE pool_address = ?
            "#
        )
        .bind(estimated_fee)
        .bind(&pool_address)
        .execute(db)
        .await?;

        updated += 1;
        println!(
            "NETWORK FEE pool={} priority={} total_estimated={}",
            pool_address,
            priority_fee,
            estimated_fee
        );

        tokio::time::sleep(
            Duration::from_millis(300)
        ).await;
    }

    println!(
        "NETWORK FEES COMPLETE updated={} failed={}",
        updated,
        failed
    );

    Ok(())
}

pub async fn run(
    db: MySqlPool,
    client: reqwest::Client,
    rpc_url: String,
    interval_seconds: u64,
) {
    let mut timer = interval(
        Duration::from_secs( interval_seconds.max(10) )
    );

    timer.set_missed_tick_behavior(
        MissedTickBehavior::Skip
    );

    loop {
        timer.tick().await;
        if let Err(error) = update_network_fees(
            &db,
            &client,
            &rpc_url,
        ).await {
            eprintln!(
                "NETWORK FEE ERROR: {error:#}"
            );
        }
    }
}

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sqlx::MySqlPool;
use std::time::Duration;
use tokio::time::{interval, MissedTickBehavior};

pub async fn get_recent_priority_fees(
    client: &reqwest::Client,
    rpc_url: &str,
) -> Result<Vec<u64>> {
    let response = client
        .post(rpc_url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getRecentPrioritizationFees",
            "params": []
        }))
        .send()
        .await?
        .error_for_status()?;

    let body: Value = response.json().await?;

    if let Some(error) = body.get("error") {
        bail!("Solana RPC: {error}");
    }

    let entries = body["result"]
        .as_array()
        .context("Invalid RPC response")?;

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

    Ok(
        samples
            .into_iter()
            .take(30)
            .map(|(_, fee)| fee)
            .collect()
    )
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
    let fees = get_recent_priority_fees(
        client,
        rpc_url,
    ).await?;

    let samples = fees.len();
    let priority_fee = percentile_75(fees)?;

    let result = sqlx::query(
        r#"
        UPDATE solana_pools
        SET
            network_fee_lamports = ?,
            network_fee_updated_at = UTC_TIMESTAMP(6)
        "#
    )
    .bind(priority_fee)
    .execute(db)
    .await?;

    println!(
        "SOLANA PRIORITY FEE \
         p75={} lamports \
         samples={} \
         pools_updated={}",
        priority_fee,
        samples,
        result.rows_affected()
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
        Duration::from_secs(
            interval_seconds.max(10)
        )
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

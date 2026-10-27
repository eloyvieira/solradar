mod dex_router;
mod jupiter_v2;
mod transaction_builder;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use futures_util::StreamExt;
use lapin::options::{BasicAckOptions, BasicConsumeOptions, BasicQosOptions};
use solradar_common::{channel, config, database, Signal};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{signature::read_keypair_file, signer::Signer};
use sqlx::{MySqlPool, Row};
use std::{env, sync::Arc, time::Duration};
use uuid::Uuid;

struct Runtime {
    http: reqwest::Client,
    rpc: RpcClient,
    wallet: solana_sdk::signature::Keypair,
    api_key: String,
    allowed_quote: String,
    amount_raw: u64,
    min_profit_raw: u64,
    network_budget_raw: u64,
    slippage_bps: u16,
    live: bool,
}

static CFG_JSON: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    serde_json::to_string(
        &std::env::vars()
            .collect::<std::collections::HashMap<String, String>>()
    ).unwrap()
});

async fn log_step(
    db: &MySqlPool,
    signal_id: &str,
    order_id: Option<&str>,
    step: &str,
    status: &str,
    details: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO trade_order_logs
            (signal_id, order_id, step, status, details, created_at)
        VALUES (?, ?, ?, ?, ?, UTC_TIMESTAMP(6))
        "#
    )
    .bind(signal_id)
    .bind(order_id)
    .bind(step)
    .bind(status)
    .bind(details)
    .execute(db)
    .await?;

    Ok(())
}

async fn update(db: &MySqlPool, order: &str, signal: &str, status: &str, signature: Option<&str>, error: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE trade_orders SET status=?, tx_signature=?, error_message=? WHERE id=?")
        .bind(status)
        .bind(signature)
        .bind(error)
        .bind(order)
        .execute(db)
        .await?;

    sqlx::query("UPDATE signals_arbitrage SET status=? WHERE id=?")
        .bind(status)
        .bind(signal)
        .execute(db)
        .await?;

    Ok(())
}

async fn process(db: &MySqlPool, cfg: &Runtime, sig: Signal) -> Result<()> {

    log_step( db, &sig.id, None,  "SIGNAL_RECEIVED", "STARTED",
        Some(CFG_JSON.as_str()),
    ).await?;

    if sig.strategy != "ARBITRAGE" { 
        bail!("A message is not arbitration"); 
    }

    let r = sqlx::query("SELECT token_address,pool_address,secondary_pool,CAST(price_a AS CHAR) pa,CAST(price_b AS CHAR) pb,observed_at,status FROM signals_arbitrage WHERE id=?")
        .bind(&sig.id)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Signal absent: {}", sig.id))?;

    let status: String = r.try_get("status")?;
    if status != "OBSERVED" { return Ok(()); }
    let token: String = r.try_get("token_address")?;
    let pool_a: String = r.try_get("pool_address")?;
    let pool_b: String = r.try_get("secondary_pool")?;
    let price_a: f64 = r.try_get::<String, _>("pa")?.parse()?;
    let price_b: f64 = r.try_get::<String, _>("pb")?.parse()?;
    let observed_at: chrono::NaiveDateTime = r.try_get("observed_at")?;

    let age = Utc::now().naive_utc().signed_duration_since(observed_at).num_seconds();
    let max_age: i64 = config("MAX_SIGNAL_AGE_SECONDS", 30_i64);
    if age < 0 || age > max_age { bail!("Signal expired age={}s", age); }

    if cfg.live {
        let security = sqlx::query("SELECT status,expires_at FROM token_security WHERE token_address=?")
            .bind(&token)
            .fetch_optional(db)
            .await?;

        let safe = security.map(|s| -> Result<bool> {
            let status: String = s.try_get("status")?;
            let expires: chrono::NaiveDateTime = s.try_get("expires_at")?;
            Ok(status == "APPROVED" && expires > Utc::now().naive_utc())
        })
        .transpose()?
        .unwrap_or(false);
        if !safe { bail!("Token without updated approval"); }
    }


    log_step( db, &sig.id, None, "POOLS_RESOLVING", "STARTED",
        Some(&serde_json::json!({
            "pool_a": pool_a,
            "pool_b": pool_b,
            "price_a": price_a,
            "price_b": price_b,
            "token": token
        }).to_string()),
    ).await?;
    let (buy_pool, sell_pool) = dex_router::resolve(db, &pool_a, &pool_b, price_a, price_b).await?;
    if buy_pool.token_address != token || buy_pool.quote_token_address != cfg.allowed_quote {
        bail!("Pair differs from the configured token/quote");
    }
    log_step( db, &sig.id, None, "POOLS_RESOLVED", "OK",
        Some(&serde_json::json!({
            "buy_pool": buy_pool.pool_address,
            "sell_pool": sell_pool.pool_address
        }).to_string()),
    ).await?;

    // Simple claim via the UNIQUE(signal_id) index. Still requires persistent
    // reconciliation of orders in SUBMITTING/UNKNOWN state before operating in production.
    let order_id = Uuid::new_v4().to_string();
    let insert = sqlx::query("INSERT IGNORE INTO trade_orders(id,signal_id,strategy,token_address,pool_address,status,mode) VALUES(?,?,?,?,?,?,?)")
        .bind(&order_id)
        .bind(&sig.id)
        .bind("ARBITRAGE")
        .bind(&token)
        .bind(&pool_a)
        .bind("PROCESSING")
        .bind(if cfg.live { "LIVE" } else { "PAPER" })
        .execute(db)
        .await?;
    if insert.rows_affected() == 0 { return Ok(()); }
    log_step( db, &sig.id, Some(&order_id), "ORDER_CREATED", "OK",
        Some(&serde_json::json!({
            "id": order_id,
            "signal_id": sig.id,
            "token": token,
            "pool": pool_a,
            "mode": if cfg.live { "LIVE" } else { "PAPER" },
            "status": "PROCESSING"
        }).to_string()),
    ).await?;


    let result: Result<()> = async {

        let wallet_address = cfg.wallet.pubkey().to_string();
        log_step( db, &sig.id, Some(&order_id), "BUY_QUOTE_REQUEST", "STARTED",
            Some(&serde_json::json!({
                "wallet_address": wallet_address,
                "input_mint": cfg.allowed_quote,
                "output_mint": token,
                "amount_raw": cfg.amount_raw,
                "slippage_bps": cfg.slippage_bps,
                "expected_pool": buy_pool.pool_address
            }).to_string()),
        ).await?;
        let buy = jupiter_v2::build(
            &cfg.http, 
            &cfg.api_key, 
            &wallet_address,
            &cfg.allowed_quote, 
            &token, 
            cfg.amount_raw, 
            cfg.slippage_bps,
        ).await?;
        log_step( db, &sig.id, Some(&order_id), "BUY_QUOTE_RECEIVED", "OK",
            Some(&serde_json::json!({
                "min_out": buy.min_out()?
            }).to_string()),
        ).await?;
        let verification = buy.verify_exact_pool(&buy_pool.pool_address, &cfg.allowed_quote, &token, cfg.amount_raw);
        log_step( db, &sig.id, Some(&order_id), "BUY_POOL_VALIDATION", if verification.is_ok() { "OK" } else { "ERROR" },
            Some(&serde_json::json!({
                "expected_pool": buy_pool.pool_address,
                "result": format!("{:?}", verification)
            }).to_string()),
        ).await?;
        verification?;

        // sells the MINIMUM amount promised by the first swap; 
        // never assumes that the optimistic outAmount will be available.
        let buy_min = buy.min_out()?;
        log_step( db, &sig.id, Some(&order_id), "SELL_QUOTE_REQUEST", "STARTED",
            Some(&serde_json::json!({
                "input_mint": token,
                "output_mint": cfg.allowed_quote,
                "amount_raw": buy_min,
                "slippage_bps": cfg.slippage_bps,
                "expected_pool": sell_pool.pool_address
            }).to_string()),
        ).await?;
        let sell = jupiter_v2::build(
            &cfg.http, 
            &cfg.api_key, 
            &wallet_address,
            &token, 
            &cfg.allowed_quote, 
            buy_min, 
            cfg.slippage_bps,
        ).await?;
        log_step( db, &sig.id, Some(&order_id), "SELL_QUOTE_RECEIVED", "OK",
            Some(&serde_json::json!({
                "min_out": sell.min_out()?
            }).to_string()),
        ).await?;
        let verification = sell.verify_exact_pool(&sell_pool.pool_address, &token, &cfg.allowed_quote, buy_min);
        log_step( db, &sig.id, Some(&order_id), "SELL_POOL_VALIDATION", if verification.is_ok() { "OK" } else { "ERROR" },
            Some(&serde_json::json!({
                "expected_pool": sell_pool.pool_address,
                "result": format!("{:?}", verification)
            }).to_string()),
        ).await?;
        verification?;

        // check the min amount
        let min_received = sell.min_out()?;
        let min_required = cfg.amount_raw
            .checked_add(cfg.min_profit_raw)
            .and_then(|n| n.checked_add(cfg.network_budget_raw))
            .context("Overflow in minimum profit")?;
        if min_received <= min_required {
            bail!("Non-profitable arbitrage in the worst case: minimum={} required={}", min_received, min_required);
        }
        println!("ARBITRAGE QUOTED id={} buy={} sell={} in={} min_out={} min_profit_raw={}",
                sig.id, 
                buy_pool.pool_address, 
                sell_pool.pool_address, 
                cfg.amount_raw, 
                min_received,
                min_received - cfg.amount_raw - cfg.network_budget_raw);

        // the priority fee is configured, not obtained as the p75 value
        // from getrecentprioritizationfees (which does not represent the total for a tx).
        let blockhash = cfg.rpc.get_latest_blockhash().await?;
        let compute_limit: u32 = config("ARBITRAGE_COMPUTE_UNIT_LIMIT", 1_300_000_u32);
        let cu_price: u64 = config("ARBITRAGE_CU_PRICE_MICROLAMPORTS", 0_u64);
        log_step( db, &sig.id, Some(&order_id), "TX_BUILD", "STARTED",
            Some(&serde_json::json!({
                "blockhash": blockhash.to_string(),
                "compute_limit": compute_limit,
                "cu_price": cu_price
            }).to_string()),
        ).await?;
        let tx = transaction_builder::atomic_transaction(
            &buy, 
            &sell, 
            &cfg.wallet, 
            blockhash, 
            compute_limit, 
            cu_price,
        )?;
        log_step( db, &sig.id, Some(&order_id), "TX_BUILD", "OK",
            Some(&serde_json::json!({
                "signature": tx.signatures.first().map(|s| s.to_string())
            }).to_string()),
        ).await?;
        log_step( db, &sig.id, Some(&order_id), "TX_SIMULATION", "STARTED",
            None,
        ).await?;
        let sim = cfg.rpc.simulate_transaction(&tx).await?;
        let sim_status = if sim.value.err.is_some() { "ERROR" } else { "OK" };
        log_step( db, &sig.id, Some(&order_id), "TX_SIMULATION", sim_status,
            Some(&serde_json::json!({
                "error": sim.value.err,
                "logs": sim.value.logs,
                "units_consumed": sim.value.units_consumed
            }).to_string()),
        ).await?;
        if let Some(err) = sim.value.err {
            bail!("Simulation rejected: {:?} logs={:?}", err, sim.value.logs);
        }
        if !cfg.live {
            update(db, &order_id, &sig.id, "SIMULATED", None, None).await?;
            log_step( db, &sig.id, Some(&order_id), "ORDER_SIMULATED", "OK",
                Some(&serde_json::json!({
                    "mode": "PAPER",
                    "status": "SIMULATED",
                    "transaction_sent": false
                }).to_string()),
            ).await?;
            return Ok(());
        }

        // signature already exists before sending. recorded prior to the network operation to
        // prevent automatic redelivery following a send timeout.
        let signature = tx.signatures.first().context("missing signature")?.to_string();
        update(db, &order_id, &sig.id, "SUBMITTING", Some(&signature), None).await?;
        log_step( db, &sig.id, Some(&order_id), "TX_SUBMITTING", "STARTED",
            Some(&serde_json::json!({
                "signature": signature,
                "mode": "LIVE"
            }).to_string()),
        ).await?;
        match cfg.rpc.send_and_confirm_transaction(&tx).await {
            Ok(confirmed) => {
                update(db, &order_id, &sig.id, "CONFIRMED", Some(&confirmed.to_string()), None).await?;
                log_step( db, &sig.id, Some(&order_id),  "TX_CONFIRMED", "OK",
                    Some(&serde_json::json!({
                        "signature": confirmed.to_string(),
                        "status": "CONFIRMED"
                    }).to_string()),
                ).await?;
            }
            Err(err) => {
                // it may have already been sent: do not automatically repeat.
                update(db, &order_id, &sig.id, "UNKNOWN", Some(&signature), Some(&err.to_string())).await?;
                eprintln!("TX UNKNOWN signature={} error={}", signature, err);
                log_step( db, &sig.id, Some(&order_id), "TX_UNKNOWN", "ERROR",
                    Some(&serde_json::json!({
                        "signature": signature,
                        "error": err.to_string(),
                        "status": "UNKNOWN"
                    }).to_string()),
                ).await?;
            }
        }
        Ok(())
    }.await;

    if let Err(ref error) = result {
        // never delete submitting / unknown statuses, nor revert them to observed.
        let row = sqlx::query("SELECT status FROM trade_orders WHERE id=?")
            .bind(&order_id)
            .fetch_one(db)
            .await?;
        let current: String = row.try_get("status")?;
        if current == "PROCESSING" {
            update(db, &order_id, &sig.id, "REJECTED", None, Some(&error.to_string())).await?;
        }
    }
    result
}

#[tokio::main]
async fn main() -> Result<()> {
    solradar_common::init();
    let db = database().await?;
    let mq = channel().await?;
    let key_path = env::var("SOLANA_KEYPAIR_PATH").context("SOLANA_KEYPAIR_PATH obrigatório")?;
    let wallet = read_keypair_file(&key_path).map_err(|e| anyhow::anyhow!("Error reading keypair{}: {}", key_path, e))?;
    let rpc_url = env::var("SOLANA_RPC_URL").context("SOLANA_RPC_URL required")?;

    let cfg = Arc::new(Runtime {
        http: reqwest::Client::builder().timeout(Duration::from_secs(15)).build()?,
        rpc: RpcClient::new(rpc_url), wallet,
        api_key: env::var("JUPITER_API_KEY").context("JUPITER_API_KEY required")?,
        allowed_quote: env::var("ARBITRAGE_ALLOWED_QUOTE_MINT").context("ARBITRAGE_ALLOWED_QUOTE_MINT required")?,
        amount_raw: env::var("ARBITRAGE_AMOUNT_RAW").context("ARBITRAGE_AMOUNT_RAW required")?.parse()?,
        min_profit_raw: env::var("ARBITRAGE_MIN_PROFIT_RAW").context("ARBITRAGE_MIN_PROFIT_RAW required")?.parse()?,
        network_budget_raw: env::var("ARBITRAGE_NETWORK_BUDGET_RAW").context("ARBITRAGE_NETWORK_BUDGET_RAW required")?.parse()?,
        slippage_bps: config("ARBITRAGE_SLIPPAGE_BPS", 30_u16),
        live: env::var("ARBITRAGE_LIVE_ENABLED").as_deref() == Ok("true"),
    });
    if cfg.amount_raw == 0 || cfg.min_profit_raw == 0 || cfg.network_budget_raw == 0 {
        bail!("Amount, min profit, and network budget must be >0");
    }

    mq.basic_qos(1, BasicQosOptions::default()).await?;
    let mut stream = mq.basic_consume( "signals.arbitrage.execute", "arbitrage-executor", BasicConsumeOptions::default(), Default::default() ).await?;
    while let Some(delivery) = stream.next().await {
        let d = delivery?;
        let work = async {
            let sig: Signal = serde_json::from_slice(&d.data)?;
            process(&db, &cfg, sig).await
        }.await;
        match work {
            Ok(()) => d.ack(BasicAckOptions::default()).await?,
            Err(err) => {
                eprintln!("ARBITRAGE ERROR: {:#}", err);
                d.ack(BasicAckOptions::default()).await?;
            }
        }
    }
    Ok(())
}

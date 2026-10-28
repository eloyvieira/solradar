mod dex_router;
mod transaction_builder;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use futures_util::StreamExt;
use lapin::options::{BasicAckOptions, BasicConsumeOptions, BasicQosOptions};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{signature::read_keypair_file, signer::Signer};
use solradar_common::{channel, config, database, Signal};
use solradar_dex_adapters::{orca::OrcaAdapter, SwapRequest};
use sqlx::{MySqlPool, Row};
use std::{env, sync::Arc};
use uuid::Uuid;

struct Runtime {
    rpc: RpcClient,
    wallet: solana_sdk::signature::Keypair,
    allowed_quote: String,
    amount_raw: u64,
    slippage_bps: u16,
    compute_limit: u32,
    compute_price: u64,
    simulate_ignore_profit: bool,
}

async fn log_step(db: &MySqlPool, signal_id: &str, order_id: Option<&str>, step: &str, status: &str, details: Option<&str>) -> Result<()> {
    sqlx::query("INSERT INTO trade_order_logs (signal_id,order_id,step,status,details,created_at) VALUES (?,?,?,?,?,UTC_TIMESTAMP(6))")
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
    sqlx::query("UPDATE trade_orders SET status=?,tx_signature=?,error_message=? WHERE id=?")
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
    if sig.strategy != "ARBITRAGE" { bail!("Unsupported strategy: {}", sig.strategy); }

    let row = sqlx::query("SELECT token_address,pool_address,secondary_pool,CAST(price_a AS CHAR) pa,CAST(price_b AS CHAR) pb,observed_at,status FROM signals_arbitrage WHERE id=?")
        .bind(&sig.id)
        .fetch_optional(db)
        .await?
        .context("Signal not found")?;

    let status: String = row.try_get("status")?;
    if status != "OBSERVED" { return Ok(()); }
    let token: String = row.try_get("token_address")?;
    let a: String = row.try_get("pool_address")?;
    let b: String = row.try_get("secondary_pool")?;
    let pa: f64 = row.try_get::<String,_>("pa")?.parse()?;
    let pb: f64 = row.try_get::<String,_>("pb")?.parse()?;
    let observed_at: chrono::NaiveDateTime = row.try_get("observed_at")?;
    let age = Utc::now().naive_utc().signed_duration_since(observed_at).num_seconds();
    let max_age: i64 = config("MAX_SIGNAL_AGE_SECONDS", 30_i64);
    if age < 0 || age > max_age { bail!("Signal stale: {} seconds", age); }

    let (buy_pool, sell_pool) = dex_router::resolve(db, &a, &b, pa, pb).await?;
    dex_router::get_adapter(&buy_pool.dex_id)?;
    dex_router::get_adapter(&sell_pool.dex_id)?;
    if  buy_pool.token_address != token 
        || sell_pool.token_address != token
        || buy_pool.quote_token_address != cfg.allowed_quote 
        || sell_pool.quote_token_address != cfg.allowed_quote {
        bail!("Unexpected token/quote pair");
    }

    let order_id = Uuid::new_v4().to_string();
    let insert = sqlx::query("INSERT IGNORE INTO trade_orders(id,signal_id,strategy,token_address,pool_address,status,mode) VALUES(?,?,?,?,?,?,?)")
        .bind(&order_id)
        .bind(&sig.id)
        .bind("ARBITRAGE")
        .bind(&token)
        .bind(&buy_pool.pool_address)
        .bind("PROCESSING")
        .bind("PAPER")
        .execute(db)
        .await?;
    if insert.rows_affected() == 0 { return Ok(()); }

    log_step(db,&sig.id,Some(&order_id),"ORDER_CREATED","OK",Some("Orca atomic simulation only; no network submission")).await?;

    let result: Result<()> = async {
        let adapter = OrcaAdapter;
        let request = SwapRequest {
            pool_address: buy_pool.pool_address.clone(),
            mint_in: cfg.allowed_quote.clone(), mint_out: token.clone(),
            amount_in_atomic: cfg.amount_raw,
        };

        log_step(db,&sig.id,Some(&order_id),"ORCA_BUY_BUILD","STARTED",Some(&request.pool_address)).await?;
        let buy = adapter.build_exact_in(&cfg.rpc,&request,cfg.wallet.pubkey(),cfg.slippage_bps).await?;

        log_step(db,&sig.id,Some(&order_id),"ORCA_BUY_QUOTE","OK",Some(&serde_json::json!({
            "min_out":buy.quote.minimum_out_atomic,
            "expected_out":buy.quote.amount_out_atomic,
            "slot":buy.quote.slot
        }).to_string())).await?;
        let buy_min = buy.quote.minimum_out_atomic;
        let sell_request = SwapRequest {
            pool_address: sell_pool.pool_address.clone(),
            mint_in: token.clone(), mint_out: cfg.allowed_quote.clone(),
            amount_in_atomic: buy_min,
        };

        log_step(db,&sig.id,Some(&order_id),"ORCA_SELL_BUILD","STARTED",Some(&sell_request.pool_address)).await?;
        let sell = adapter.build_exact_in(&cfg.rpc,&sell_request,cfg.wallet.pubkey(),cfg.slippage_bps).await?;

        log_step(db,&sig.id,Some(&order_id),"ORCA_SELL_QUOTE","OK",Some(&serde_json::json!({
            "min_out":sell.quote.minimum_out_atomic,
            "expected_out":sell.quote.amount_out_atomic,
            "slot":sell.quote.slot
        })
        .to_string()))
        .await?;

        let blockhash = cfg.rpc.get_latest_blockhash().await?;
        let min_out = sell.quote.minimum_out_atomic;
        if min_out <= cfg.amount_raw && !cfg.simulate_ignore_profit {
            bail!("Worst-case unprofitable: input={} return={}; set ORCA_TEST_IGNORE_PROFIT=true for simulation only",cfg.amount_raw,min_out);
        }

        log_step(db,&sig.id,Some(&order_id),"ATOMIC_TX_BUILD","STARTED",None).await?;
        let tx = transaction_builder::atomic_transaction(
            &buy.instructions,
            &sell.instructions,
            &[],
            &cfg.wallet,blockhash,
            cfg.compute_limit,
            cfg.compute_price,
        )?;

        log_step(db,&sig.id,Some(&order_id),"ATOMIC_TX_BUILD","OK",Some(&serde_json::json!({
            "signature":tx.signatures.first().map(|v|v.to_string()),
            "buy_ixs":buy.instructions.len(),
            "sell_ixs":sell.instructions.len()
        })
        .to_string()))
        .await?;
        
        let sim = cfg.rpc.simulate_transaction(&tx).await?;
        let error = sim.value.err.as_ref().map(|e|format!("{e:?}"));
        log_step(db,&sig.id,Some(&order_id),"ATOMIC_SIMULATION",if error.is_some(){"ERROR"}else{"OK"},Some(&serde_json::json!({
            "error":error,
            "logs":sim.value.logs,
            "units_consumed":sim.value.units_consumed,
            "input_atomic":cfg.amount_raw,
            "minimum_final_atomic":min_out,
            "sent":false
        })
        .to_string()))
        .await?;

        if let Some(err)=error { 
            bail!("Atomic simulation failed: {err}"); 
        }
        update(db,&order_id,&sig.id,"SIMULATED",None,None).await?;

        Ok(())
    }.await;
    if let Err(ref e)=result {
        let row = sqlx::query("SELECT status FROM trade_orders WHERE id=?")
                        .bind(&order_id)
                        .fetch_one(db)
                        .await?;

        let current: String = row.try_get("status")?;
        if current=="PROCESSING" {
            update(db,&order_id,&sig.id,"REJECTED",None,Some(&e.to_string())).await?;
        }
        log_step(db,&sig.id,Some(&order_id),"ARBITRAGE_TEST_ERROR","ERROR",Some(&e.to_string())).await?;
    }
    result
}

#[tokio::main]
async fn main()->Result<()> {
    solradar_common::init();
    if env::var("ARBITRAGE_LIVE_ENABLED").as_deref()==Ok("true") {
        bail!("LIVE disabled in Orca atomic simulation prototype");
    }
    let cfg=Arc::new(Runtime {
        rpc: RpcClient::new(env::var("SOLANA_RPC_URL").context("SOLANA_RPC_URL required")?),
        wallet: read_keypair_file(env::var("SOLANA_KEYPAIR_PATH").context("SOLANA_KEYPAIR_PATH required")?).map_err(|e|anyhow::anyhow!("Wallet read error: {e}"))?,
        allowed_quote: env::var("ARBITRAGE_ALLOWED_QUOTE_MINT").context("ARBITRAGE_ALLOWED_QUOTE_MINT required")?,
        amount_raw: env::var("ARBITRAGE_AMOUNT_RAW").context("ARBITRAGE_AMOUNT_RAW required")?.parse()?,
        slippage_bps: config("ARBITRAGE_SLIPPAGE_BPS",30_u16),
        compute_limit: config("ARBITRAGE_COMPUTE_UNIT_LIMIT",1_300_000_u32),
        compute_price: config("ARBITRAGE_CU_PRICE_MICROLAMPORTS",0_u64),
        simulate_ignore_profit: env::var("ORCA_TEST_IGNORE_PROFIT").as_deref()==Ok("true"),
    });
    if cfg.amount_raw==0 || cfg.slippage_bps>10_000 {bail!("Invalid amount or slippage")}
    let db=database().await?;
    let mq=channel().await?;
    mq.basic_qos(1,BasicQosOptions::default()).await?;
    let mut stream=mq.basic_consume("signals.arbitrage.execute","orca-atomic-test",BasicConsumeOptions::default(),Default::default()).await?;
    while let Some(delivery)=stream.next().await {
        let d=delivery?;
        let work=async {let sig:Signal=serde_json::from_slice(&d.data)?;process(&db,&cfg,sig).await}.await;
        if let Err(e)=work {eprintln!("ORCA ATOMIC TEST ERROR: {e:#}");}
        d.ack(BasicAckOptions::default()).await?;
    }
    Ok(())
}

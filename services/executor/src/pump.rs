use anyhow::{bail, Context, Result};
use chrono::{NaiveDateTime, Utc};
use solradar_common::{config};
use sqlx::{MySqlPool, Row};
use std::{env, time::Duration};
use uuid::Uuid;

const WSOL_MINT: &str = "So11111111111111111111111111111111111111112";

struct Settings {
    usdt_mint: String,
    investment_usd: f64,
    take_profit_pct: f64,
    stop_loss_pct: f64,
    max_signal_age_seconds: i64,
    max_price_age_seconds: i64,
    poll_seconds: u64,
}

impl Settings {
    fn from_env() -> Result<Self> {
        if env::var("PUMP_EXECUTOR_MODE").as_deref() != Ok("PAPER") {
            bail!("PUMP_EXECUTOR_MODE=PAPER is required; LIVE is not implemented");
        }
        let settings = Self {
            usdt_mint: env::var("PUMP_USDT_MINT").context("PUMP_USDT_MINT required")?,
            investment_usd: config("PUMP_ORDER_USD", 10.0_f64),
            take_profit_pct: config("PUMP_TAKE_PROFIT_PCT", 2.0_f64),
            stop_loss_pct: config("PUMP_STOP_LOSS_PCT", 2.0_f64),
            max_signal_age_seconds: config("PUMP_MAX_SIGNAL_AGE_SECONDS", 90_i64),
            max_price_age_seconds: config("PUMP_MAX_PRICE_AGE_SECONDS", 120_i64),
            poll_seconds: config("PUMP_EXECUTOR_POLL_SECONDS", 5_u64),
        };
        if settings.usdt_mint.trim().is_empty() || settings.usdt_mint == WSOL_MINT {
            bail!("Invalid PUMP_USDT_MINT");
        }
        if !settings.investment_usd.is_finite() || settings.investment_usd <= 0.0
            || !settings.take_profit_pct.is_finite() || settings.take_profit_pct <= 0.0
            || !settings.stop_loss_pct.is_finite() || !(0.0..100.0).contains(&settings.stop_loss_pct)
            || settings.max_signal_age_seconds <= 0 || settings.max_price_age_seconds <= 0
            || settings.poll_seconds == 0
        {
            bail!("Invalid PUMP executor configuration");
        }
        Ok(settings)
    }
}

fn read_price(row: &sqlx::mysql::MySqlRow) -> Result<f64> {
    let str_price: String = row.try_get("price")?;
    let price = str_price.parse::<f64>()?;
    if !price.is_finite() || price <= 0.0 {
        bail!("Invalid pool price: {str_price}");
    }
    Ok(price)
}

fn fresh(ts: NaiveDateTime, max_age_seconds: i64) -> bool {
    let age = Utc::now().naive_utc().signed_duration_since(ts).num_seconds();
    (0..=max_age_seconds).contains(&age)
}

async fn latest_price(db: &MySqlPool, pool: &str) -> Result<Option<(f64, NaiveDateTime)>> {
    let row = sqlx::query(
        r#"SELECT CAST(price_usd AS CHAR) AS price, captured_at
           FROM solana_pool_snapshots
           WHERE pool_address = ?
           ORDER BY captured_at DESC
           LIMIT 1"#,
    )
    .bind(pool)
    .fetch_optional(db)
    .await?;
    row.map(|r| -> Result<_> {
        Ok((read_price(&r)?, r.try_get::<NaiveDateTime, _>("captured_at")?))
    })
    .transpose()
}

async fn open_new_positions(db: &MySqlPool, cfg: &Settings) -> Result<()> {
    // A tabela signals_pump existente não tem coluna status.
    // O índice UNIQUE(signal_id) em pump_positions garante idempotência por sinal.
    let signals = sqlx::query(
        r#"SELECT s.id, s.token_address, s.pool_address, s.observed_at,
                  p.quote_token_address
           FROM signals_pump s
           INNER JOIN solana_pools p ON p.pool_address = s.pool_address
           LEFT JOIN pump_positions pos ON pos.signal_id = s.id
           WHERE pos.signal_id IS NULL
             AND s.observed_at >= DATE_SUB(UTC_TIMESTAMP(), INTERVAL ? SECOND)
           ORDER BY s.observed_at ASC
           LIMIT 50"#,
    )
    .bind(cfg.max_signal_age_seconds)
    .fetch_all(db)
    .await?;

    for signal in signals {
        let signal_id: String = signal.try_get("id")?;
        let token: String = signal.try_get("token_address")?;
        let pool: String = signal.try_get("pool_address")?;
        let quote: Option<String> = signal.try_get("quote_token_address")?;
        let observed_at: NaiveDateTime = signal.try_get("observed_at")?;
        if !fresh(observed_at, cfg.max_signal_age_seconds) {
            continue;
        }
        let Some(quote) = quote else { continue };
        if quote != cfg.usdt_mint && quote != WSOL_MINT {
            continue;
        }
        if token == quote { continue; }
        let Some((price, captured_at)) = latest_price(db, &pool).await? else {
            continue;
        };
        if captured_at < observed_at || !fresh(captured_at, cfg.max_price_age_seconds) {
            continue;
        }

        // Uma posição aberta por token neste serviço (processo único).
        let has_open = sqlx::query(
            "SELECT id FROM pump_positions WHERE token_address = ? AND status = 'OPEN' LIMIT 1",
        )
        .bind(&token)
        .fetch_optional(db)
        .await?
        .is_some();
        if has_open { continue; }

        let pair = if quote == WSOL_MINT { "SOL" } else { "USDT" };
        let position_id = Uuid::new_v4().to_string();
        let result = sqlx::query(
            r#"INSERT IGNORE INTO pump_positions
               (id, signal_id, token_address, pool_address, quote_token_address,
                quote_symbol, mode, status, amount_usd, entry_price_usd,
                take_profit_price_usd, stop_loss_price_usd, opened_at,
                last_price_usd, last_checked_at)
               VALUES (?, ?, ?, ?, ?, ?, 'PAPER', 'OPEN', ?, ?, ?, ?,
                       UTC_TIMESTAMP(6), ?, UTC_TIMESTAMP(6))"#,
        )
        .bind(&position_id)
        .bind(&signal_id)
        .bind(&token)
        .bind(&pool)
        .bind(&quote)
        .bind(pair)
        .bind(cfg.investment_usd)
        .bind(price)
        .bind(price * (1.0 + cfg.take_profit_pct / 100.0))
        .bind(price * (1.0 - cfg.stop_loss_pct / 100.0))
        .bind(price)
        .execute(db)
        .await?;
        if result.rows_affected() > 0 {
            println!("PUMP PAPER OPEN position={position_id} token={token} pair=/{pair} entry_usd={price:.12} budget_usd={:.2}", cfg.investment_usd);
        }
    }
    Ok(())
}

async fn monitor_open_positions(db: &MySqlPool, cfg: &Settings) -> Result<()> {
    let positions = sqlx::query(
        r#"SELECT id, pool_address, CAST(entry_price_usd AS CHAR) AS entry,
                  CAST(take_profit_price_usd AS CHAR) AS tp,
                  CAST(stop_loss_price_usd AS CHAR) AS sl
           FROM pump_positions
           WHERE status = 'OPEN' AND mode = 'PAPER'
           ORDER BY opened_at ASC
           LIMIT 500"#,
    )
    .fetch_all(db)
    .await?;

    for pos in positions {
        let id: String = pos.try_get("id")?;
        let pool: String = pos.try_get("pool_address")?;
        let entry: f64 = pos.try_get::<String, _>("entry")?.parse()?;
        let tp: f64 = pos.try_get::<String, _>("tp")?.parse()?;
        let sl: f64 = pos.try_get::<String, _>("sl")?.parse()?;
        if entry <= 0.0 { continue; }
        let Some((price, captured_at)) = latest_price(db, &pool).await? else { continue };
        if !fresh(captured_at, cfg.max_price_age_seconds) { continue; }

        let close_status = if price >= tp {
            Some("CLOSED_TP")
        } else if price <= sl {
            Some("CLOSED_SL")
        } else {
            None
        };
        let roi_pct = (price / entry - 1.0) * 100.0;
        if let Some(status) = close_status {
            // Apenas um monitor marca o encerramento; nenhuma ordem é enviada.
            let result = sqlx::query(
                r#"UPDATE pump_positions
                   SET status=?, exit_price_usd=?, pnl_pct=?, closed_at=UTC_TIMESTAMP(6),
                       last_price_usd=?, last_checked_at=UTC_TIMESTAMP(6)
                   WHERE id=? AND status='OPEN'"#,
            )
            .bind(status)
            .bind(price)
            .bind(roi_pct)
            .bind(price)
            .bind(&id)
            .execute(db)
            .await?;
            if result.rows_affected() > 0 {
                println!("PUMP PAPER {status} position={id} exit_usd={price:.12} pnl_pct={roi_pct:.4}");
            }
        } else {
            sqlx::query(
                r#"UPDATE pump_positions
                   SET last_price_usd=?, last_checked_at=UTC_TIMESTAMP(6)
                   WHERE id=? AND status='OPEN'"#,
            )
            .bind(price)
            .bind(&id)
            .execute(db)
            .await?;
        }
    }
    Ok(())
}

pub async fn run(db: MySqlPool) -> Result<()> {
    let cfg = Settings::from_env()?;
    let db = database().await?;
    println!("Pump Executor started: PAPER only; no real transactions");

    loop {
        if let Err(err) = monitor_open_positions(&db, &cfg).await {
            eprintln!("pump monitor error: {err:#}");
        }
        if let Err(err) = open_new_positions(&db, &cfg).await {
            eprintln!("pump open error: {err:#}");
        }
        tokio::time::sleep(Duration::from_secs(cfg.poll_seconds)).await;
    }
}

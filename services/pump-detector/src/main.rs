use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use lapin::options::{BasicAckOptions, BasicConsumeOptions, BasicNackOptions, BasicQosOptions};
use solradar_common::{channel, config, database, PoolEvent};
use sqlx::Row;
use std::{env, time::Duration};
use uuid::Uuid;

const WSOL_MINT: &str = "So11111111111111111111111111111111111111112";

#[tokio::main]
async fn main() -> Result<()> {
    solradar_common::init();

    // Não confundir USDT com USDC: exigir explicitamente o mint de USDT.
    let usdt_mint = env::var("PUMP_USDT_MINT")
        .context("PUMP_USDT_MINT must contain the Solana USDT mint address")?;
    if usdt_mint.trim().is_empty() || usdt_mint == WSOL_MINT {
        bail!("Invalid PUMP_USDT_MINT");
    }

    let db = database().await?;
    let mq = channel().await?;
    mq.basic_qos(20, BasicQosOptions::default()).await?;

    let mut stream = mq
        .basic_consume(
            "market.pump",
            "pump-detector",
            BasicConsumeOptions::default(),
            Default::default(),
        )
        .await?;

    while let Some(delivery) = stream.next().await {
        let d = delivery?;

        let outcome: Result<()> = async {
            let event: PoolEvent = serde_json::from_slice(&d.data)?;

            // Campos e filtros originais mantidos. Join para conhecer o par real.
            let row = sqlx::query(
                r#"
                SELECT
                    p.token_address,
                    p.quote_token_address,
                    CAST(s.price_usd AS CHAR) AS price,
                    CAST(s.volume_m5_usd AS CHAR) AS v,
                    CAST(s.liquidity_usd AS CHAR) AS l,
                    CAST(s.volume_acceleration_5m AS CHAR) AS a,
                    CAST(s.change_m5_pct AS CHAR) AS c5,
                    CAST(s.change_m15_pct AS CHAR) AS c15,
                    CAST(s.change_m30_pct AS CHAR) AS c30,
                    CAST(s.change_h1_pct AS CHAR) AS c1h,
                    s.buys_m5,
                    s.sells_m5
                FROM solana_pool_snapshots s
                INNER JOIN solana_pools p
                    ON p.pool_address = s.pool_address
                WHERE s.pool_address = ?
                  AND s.captured_at = ?
                LIMIT 1
                "#,
            )
            .bind(&event.pool_address)
            .bind(event.captured_at.naive_utc())
            .fetch_optional(&db)
            .await?;

            let Some(r) = row else { return Ok(()); };

            let token: String = r.try_get("token_address")?;
            let quote: Option<String> = r.try_get("quote_token_address")?;
            let Some(quote) = quote else { return Ok(()); };

            if token != event.token_address || token == quote {
                return Ok(());
            }
            if quote != usdt_mint && quote != WSOL_MINT {
                return Ok(());
            }

            let decimal = |field: &str| -> Option<f64> {
                r.try_get::<Option<String>, _>(field)
                    .ok()
                    .flatten()?
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
            };

            let Some(price) = decimal("price").filter(|v| *v > 0.0) else {
                return Ok(());
            };
            let v = decimal("v").unwrap_or(0.0);
            let l = decimal("l").unwrap_or(0.0);
            let a = decimal("a").unwrap_or(0.0);
            let c5 = decimal("c5");
            let c15 = decimal("c15");
            let c30 = decimal("c30");
            let c1h = decimal("c1h");

            let buys: u32 = r.try_get("buys_m5")?;
            let sells: u32 = r.try_get("sells_m5")?;
            let ratio = solradar_market_models::buy_count_ratio(buys, sells)
                .unwrap_or(0.0);

            let history = sqlx::query(
                r#"
                SELECT
                    CAST(MAX(price_usd) AS CHAR) AS max_price,
                    COUNT(*) AS total_snapshots
                FROM solana_pool_snapshots
                WHERE pool_address = ?
                  AND captured_at >= DATE_SUB(?, INTERVAL 60 MINUTE)
                  AND captured_at < ?
                "#,
            )
            .bind(&event.pool_address)
            .bind(event.captured_at.naive_utc())
            .bind(event.captured_at.naive_utc())
            .fetch_one(&db)
            .await?;

            let max_price = history
                .try_get::<Option<String>, _>("max_price")?
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite())
                .unwrap_or(0.0);
            let total_snapshots: i64 = history.try_get("total_snapshots")?;

            let volume_ok = a >= config("PUMP_MIN_ACCEL", 2.0)
                && v >= config("PUMP_MIN_VOLUME_M5_USD", 10_000.0)
                && ratio >= config("PUMP_MIN_BUY_RATIO", 0.60);

            let trend_ok = c5.is_some_and(|x| x >= config("PUMP_MIN_CHANGE_M5_PCT", 2.0))
                && c15.is_some_and(|x| x >= config("PUMP_MIN_CHANGE_M15_PCT", 2.0))
                && c30.is_some_and(|x| x >= config("PUMP_MIN_CHANGE_M30_PCT", 4.0))
                && c1h.is_some_and(|x| x > 0.0);

            let drawdown_ok = max_price > 0.0 && price >= max_price * 0.85;
            let history_ok = total_snapshots >= 10;
            let liquidity_ok = l >= config("PUMP_MIN_LIQUIDITY_USD", 50_000.0);

            if !(volume_ok && trend_ok && drawdown_ok && history_ok && liquidity_ok) {
                return Ok(());
            }

            // Cooldown por token entre pools USDT/SOL; evita sinais repetidos.
            let cooldown_minutes: i64 = config("PUMP_COOLDOWN_MINUTES", 15_i64);
            if !(1..=1440).contains(&cooldown_minutes) {
                bail!("PUMP_COOLDOWN_MINUTES must be between 1 and 1440");
            }

            let recent = sqlx::query(
                r#"
                SELECT id
                FROM signals_pump
                WHERE token_address = ?
                  AND observed_at BETWEEN DATE_SUB(?, INTERVAL ? MINUTE) AND ?
                LIMIT 1
                "#,
            )
            .bind(&token)
            .bind(event.captured_at.naive_utc())
            .bind(cooldown_minutes)
            .bind(event.captured_at.naive_utc())
            .fetch_optional(&db)
            .await?;

            if recent.is_some() {
                return Ok(());
            }

            let pair = if quote == WSOL_MINT { "SOL" } else { "USDT" };
            let id = Uuid::new_v4().to_string();

            // TP/SL são referências para o futuro executor PAPER;
            // ainda NÃO representam uma compra executada.
            let tp_pct = 2.0_f64;
            let sl_pct = 2.0_f64;
            let reason = format!(
                "mode=PAPER; pair={pair}; quote_mint={quote}; \
                 entry_reference_usd={price:.12}; tp_pct={tp_pct:.2}; \
                 sl_pct={sl_pct:.2}; tp_reference_usd={:.12}; \
                 sl_reference_usd={:.12}; order_budget_usdt=10.00; \
                 acceleration={a:.2}; change_m5={c5:?}; \
                 change_m15={c15:?}; change_m30={c30:?}; \
                 change_h1={c1h:?}; buy_ratio={ratio:.3}; \
                 volume_m5={v:.2}; liquidity_usd={l:.2}; \
                 max_price_60m={max_price:.12}",
                price * 1.02,
                price * 0.98,
            );

            let res = sqlx::query(
                r#"
                INSERT IGNORE INTO signals_pump (
                    id, token_address, pool_address, observed_at, reason
                )
                VALUES (?, ?, ?, ?, ?)
                "#,
            )
            .bind(&id)
            .bind(&token)
            .bind(&event.pool_address)
            .bind(event.captured_at.naive_utc())
            .bind(&reason)
            .execute(&db)
            .await?;

            if res.rows_affected() > 0 {
                // Não publicar em "signals.execute": a fila atual é
                // usada pelo Executor de arbitragem.
                println!(
                    "PUMP PAPER SAVED token={} pair={} pool={} reference_price_usd={:.12}",
                    token, pair, event.pool_address, price
                );
            }
            Ok(())
        }
        .await;

        match outcome {
            Ok(()) => d.ack(BasicAckOptions::default()).await?,
            Err(err) => {
                eprintln!("pump error: {err:#}");
                d.nack(BasicNackOptions {
                    requeue: true,
                    ..Default::default()
                })
                .await?;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }

    Ok(())
}

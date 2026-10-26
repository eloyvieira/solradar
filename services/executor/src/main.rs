mod dex_router;
mod jupiter_v2;
mod transaction_builder;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use futures_util::StreamExt;
use lapin::options::{ BasicAckOptions, BasicConsumeOptions, BasicNackOptions, BasicQosOptions,};
use solradar_common::{channel, config, database, Signal};
use sqlx::Row;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    solradar_common::init();

    let db = database().await?;
    let mq = channel().await?;

    mq.basic_qos( 10, BasicQosOptions::default(),).await?;

    let mut stream = mq.basic_consume(
        "signals.arbitrage.execute",
        "arbitrage-executor",
        BasicConsumeOptions::default(),
        Default::default(),
    ).await?;

    while let Some(delivery) = stream.next().await {
        let d = delivery?;

        let work: Result<()> = async {
            let sig: Signal = serde_json::from_slice(&d.data)?;
            if sig.strategy != "ARBITRAGE" {
                bail!("Unsupported strategy");
            }

            // validate signal
            let signal_row = sqlx::query(
                r#"
                SELECT
                    id,
                    status,
                    token_address,
                    pool_address,
                    secondary_pool,
                    CAST(price_a AS CHAR) AS price_a,
					CAST(price_b AS CHAR) AS price_b,
                    observed_at
                FROM signals_arbitrage
                WHERE id = ?
                "#
            )
            .bind(&sig.id)
            .fetch_optional(&db)
            .await?;
            let signal_row = match signal_row {
                Some(row) => row,
                None => {
                    println!(
                        "ARBITRAGE SIGNAL NOT FOUND {}",
                        sig.id
                    );
                    return Ok(());
                }
            };

            // check if it has already been executed
            let signal_status: String = signal_row.get("status");
            if signal_status != "OBSERVED" {
                println!(
                    "ARBITRAGE ALREADY PROCESSED \
                     signal={} status={}",
                    sig.id,
                    signal_status
                );
                return Ok(());
            }

            // check if it has already been executed
            let token_address: String = signal_row.get("token_address");
            let pool_address: String = signal_row.get("pool_address");
            let secondary_pool: String = signal_row.get("secondary_pool");
            let observed_at: chrono::NaiveDateTime = signal_row.get("observed_at");

            // check invalid prices or pools
            let price_a: f64 = signal_row
			    .try_get::<String, _>("price_a")?
			    .parse()?;
			let price_b: f64 = signal_row
			    .try_get::<String, _>("price_b")?
			    .parse()?;
			let (buy_pool, sell_pool) =
			    dex_router::resolve_arbitrage(
			        &db,
			        &pool_address,
			        &secondary_pool,
			        price_a,
			        price_b,
			    )
			    .await?;
			println!(
			    "ARBITRAGE ROUTE \
			     buy_dex={} buy_pool={} \
			     sell_dex={} sell_pool={}",
			    buy_pool.dex_id,
			    buy_pool.pool_address,
			    sell_pool.dex_id,
			    sell_pool.pool_address
			);

            // token security
            let security = sqlx::query(
                r#"
                SELECT status, expires_at
                FROM token_security
                WHERE token_address = ?
                "#
            )
            .bind(&token_address)
            .fetch_optional(&db)
            .await?;

            let safe = security.map(|row| {
                let status: String = row.get("status");
                let expiry: chrono::NaiveDateTime = row.get("expires_at");
                status == "APPROVED" && expiry > Utc::now().naive_utc()
            })
            .unwrap_or(false);

            // validity of the signal
            let age = Utc::now().naive_utc().signed_duration_since(observed_at).num_seconds();
            let max_age = config( "MAX_SIGNAL_AGE_SECONDS", 30_i64,);
            let status = if !safe {
                "REJECTED_SECURITY"
            } else if age < 0 || age > max_age {
                "STALE_SIGNAL"
            } else {
                "PENDING_VALIDATION"
            };

            // register order only once
            let order_id = Uuid::new_v4().to_string();
            let res = sqlx::query(
                r#"
                INSERT IGNORE INTO trade_orders (
                    id,
                    signal_id,
                    strategy,
                    token_address,
                    pool_address,
                    status,
                    mode,
                    error_message
                )
                VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                "#
            )
            .bind(&order_id)
            .bind(&sig.id)
            .bind("ARBITRAGE")
            .bind(&token_address)
            .bind(&pool_address)
            .bind(status)
            .bind("PAPER")
            .bind(if status == "PENDING_VALIDATION" {
                None
            } else {
                Some("Security or signal age rejected")
            })
            .execute(&db)
            .await?;
            if res.rows_affected() == 0 {
                println!(
                    "ARBITRAGE DUPLICATE ORDER \
                     signal={}",
                    sig.id
                );

                return Ok(());
            }

            // update signal status
            sqlx::query(
                r#"
                UPDATE signals_arbitrage
                SET status = ?
                WHERE id = ?
                "#
            )
            .bind(status)
            .bind(&sig.id)
            .execute(&db)
            .await?;

            // publish event
            publish(
                &mq,
                "ledger.events",
                &LedgerEvent {
                    order_id: order_id.clone(),
                    event_type: "ORDER_RECORDED".into(),
                },
            ).await?;
            println!(
                "ARBITRAGE EXECUTOR \
                 signal={} \
                 token={} \
                 pool_a={} \
                 pool_b={} \
                 status={} \
                 age={}s",
                sig.id,
                token_address,
                pool_address,
                secondary_pool,
                status,
                age
            );
            Ok(())
        }.await;

        match work {
            Ok(()) => {
                d.ack(
                    BasicAckOptions::default()
                ).await?;
            }

            Err(error) => {
                eprintln!(
                    "ARBITRAGE EXECUTOR ERROR: {error:#}"
                );

                d.nack(
                    BasicNackOptions {
                        requeue: true,
                        ..Default::default()
                    }
                ).await?;

                tokio::time::sleep(
                    std::time::Duration::from_secs(2)
                ).await;
            }
        }
    }

    Ok(())
}

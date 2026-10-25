use anyhow::Result;
use futures_util::StreamExt;
use lapin::options::{BasicAckOptions,BasicConsumeOptions,BasicQosOptions};
use solradar_common::{database,channel,publish,PoolEvent,Signal,config};
use sqlx::Row;
use uuid::Uuid;

#[tokio::main]async fn main()->Result<()>{
	solradar_common::init();
	let db=database().await?;
	let mq=channel().await?;
	mq.basic_qos(20,BasicQosOptions::default()).await?;
	
	let mut stream=mq.basic_consume("market.pump","pump-detector",BasicConsumeOptions::default(),Default::default()).await?;
	while 
	let Some(delivery)=stream.next().await{
		let d=delivery?;
		let outcome:Result<()> = async {
			let event:PoolEvent=serde_json::from_slice(&d.data)?;
			let row = sqlx::query(
			    r#"
			    SELECT
			        CAST(price_usd AS CHAR) AS price,
			        CAST(volume_m5_usd AS CHAR) AS v,
			        CAST(liquidity_usd AS CHAR) AS l,
			        CAST(volume_acceleration_5m AS CHAR) AS a,

			        CAST(change_m5_pct AS CHAR) AS c5,
			        CAST(change_m15_pct AS CHAR) AS c15,
			        CAST(change_m30_pct AS CHAR) AS c30,
			        CAST(change_h1_pct AS CHAR) AS c1h,

			        buys_m5,
			        sells_m5,
			        buys_m15,
			        sells_m15

			    FROM
			        solana_pool_snapshots

			    WHERE
			        pool_address = ?
			        AND captured_at = ?
			    "#
			)
			.bind(&event.pool_address)
			.bind(event.captured_at.naive_utc())
			.fetch_optional(&db)
			.await?;


			if let Some(r)=row{
				let decimal = |field: &str| -> Option<f64> {
				    let value = r
				        .try_get::<Option<String>, _>(field)
				        .ok()
				        .flatten()?;
				    value.parse::<f64>().ok().filter(|v| v.is_finite())
				};
				let price = decimal("price").unwrap_or(0.0);
				let v:f64=r.get::<String,_>("v").parse().unwrap_or(0.);
				let l:f64=r.get::<String,_>("l").parse().unwrap_or(0.);
				let a:f64=r.try_get::<Option<String>,_>("a")?.unwrap_or_default().parse().unwrap_or(0.);

				let c5 = decimal("c5");
				let c15 = decimal("c15");
				let c30 = decimal("c30");
				let c1h = decimal("c1h");

				let buys: u32 = r.get("buys_m5");
				let sells: u32 = r.get("sells_m5");

				let ratio=solradar_market_models::buy_count_ratio(buys,sells).unwrap_or(0.0);

				let history = sqlx::query(
				    r#"
				    SELECT
				        CAST(MAX(price_usd) AS CHAR) AS max_price,
				        CAST(MIN(price_usd) AS CHAR) AS min_price,
				        COUNT(*) AS total_snapshots
				    FROM
				        solana_pool_snapshots
				    WHERE
				        pool_address = ?
				        AND captured_at >= DATE_SUB(?, INTERVAL 60 MINUTE)
				        AND captured_at < ?
				    "#
				)
				.bind(&event.pool_address)
				.bind(event.captured_at.naive_utc())
				.bind(event.captured_at.naive_utc())
				.fetch_one(&db)
				.await?;

				let max_price = history
				    .try_get::<Option<String>, _>("max_price")?
				    .and_then(|v| v.parse::<f64>().ok())
				    .unwrap_or(0.0);

				let total_snapshots: i64 = history.get("total_snapshots");

				let volume_ok = a >= config("PUMP_MIN_ACCEL", 2.0)
				    			&& v >= config("PUMP_MIN_VOLUME_M5_USD", 10_000.0)
				    			&& ratio >= config("PUMP_MIN_BUY_RATIO", 0.60);

				let trend_ok = c5.is_some_and(|x| x >= 2.0)
							    && c15.is_some_and(|x| x >= 2.0)
							    && c30.is_some_and(|x| x >= 4.0)
							    && c1h.is_some_and(|x| x > 0.0);

				let drawdown_ok = max_price > 0.0 
								&& price >= max_price * 0.85;
				let history_ok = total_snapshots >= 10;

				let liquidity_ok = l >= config("PUMP_MIN_LIQUIDITY_USD", 50_000.0);

				let is_pump = volume_ok
							    && trend_ok
							    && drawdown_ok
							    && history_ok
							    && liquidity_ok;

				if is_pump {

				    let signal = Signal {
				        id: Uuid::new_v4().to_string(),
				        strategy: "PUMP".into(),
				        pool_address: event.pool_address,
				        secondary_pool: None,
				        token_address: event.token_address,
				        observed_at: event.captured_at,
				        reason: format!(
				            "acceleration={a:.2}; \
							change_m5={c5:?}; \
							change_m15={c15:?}; \
							change_m30={c30:?}; \
							change_h1={c1h:?}; \
							buy_ratio={ratio:.3}; \
							volume_m5={v:.2}; \
							max_price_60m={max_price:.12}"
				        ),
				    };

				    let res = sqlx::query(
				        r#"
				        INSERT IGNORE INTO signals_pump (
				            id,
				            strategy,
				            token_address,
				            pool_address,
				            secondary_pool,
				            observed_at,
				            reason
				        )
				        VALUES (?, ?, ?, ?, ?, ?, ?)
				        "#
				    )
				    .bind(&signal.id)
				    .bind(&signal.strategy)
				    .bind(&signal.token_address)
				    .bind(&signal.pool_address)
				    .bind(&signal.secondary_pool)
				    .bind(signal.observed_at.naive_utc())
				    .bind(&signal.reason)
				    .execute(&db)
				    .await?;

				    if res.rows_affected() > 0 {
				        publish(&mq, "signals.execute", &signal).await?;

				        println!("PUMP CANDIDATE {}", signal.token_address);
				    }
				}
			}
			Ok(())
		}
		.await;

		match outcome{
			Ok(())=>d.ack(BasicAckOptions::default()).await?,Err(e)=>{
				eprintln!("pump error: {e:#}");
				d.nack(
					lapin::options::BasicNackOptions{requeue:true,..Default::default()}
				)
				.await?;
				tokio::time::sleep(std::time::Duration::from_secs(2)).await;
			}
		}
	}
	Ok(())
}

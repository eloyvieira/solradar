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
			        CAST(volume_m5_usd AS CHAR) AS v,
			        CAST(liquidity_usd AS CHAR) AS l,
			        CAST(change_m5_pct AS CHAR) AS c,
			        CAST(volume_acceleration_5m AS CHAR) AS a,
			        buys_m5,
			        sells_m5
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
				let v:f64=r.get::<String,_>("v").parse().unwrap_or(0.);
				let l:f64=r.get::<String,_>("l").parse().unwrap_or(0.);
				let c:f64=r.get::<String,_>("c").parse().unwrap_or(0.);
				let a:f64=r.try_get::<Option<String>,_>("a")?.unwrap_or_default().parse().unwrap_or(0.);
				let b:u32=r.get("buys_m5");
				let s:u32=r.get("sells_m5");
				let ratio=solradar_market_models::buy_count_ratio(b,s).unwrap_or(0.);

				if  a >= config("PUMP_MIN_ACCEL", 2.0)
				    && c >= config("PUMP_MIN_CHANGE_PCT", 2.0)
				    && ratio >= config("PUMP_MIN_BUY_RATIO", 0.60)
				    && l >= config("PUMP_MIN_LIQUIDITY_USD", 50_000.0)
				    && v >= config("PUMP_MIN_VOLUME_M5_USD", 10_000.0)
				{
				    let signal = Signal {
				        id: Uuid::new_v4().to_string(),
				        strategy: "PUMP".into(),
				        pool_address: event.pool_address,
				        secondary_pool: None,
				        token_address: event.token_address,
				        observed_at: event.captured_at,
				        reason: format!(
				            "acceleration={a:.2}; change={c:.2}; buy_count_ratio={ratio:.3}; volume_m5={v:.2}"
				        ),
				    };

				    let res = sqlx::query(
				        r#"
				        INSERT IGNORE INTO trade_signals (
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

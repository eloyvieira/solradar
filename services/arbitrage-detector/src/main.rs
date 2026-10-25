use anyhow::Result;
use futures_util::StreamExt;
use lapin::options::{BasicAckOptions,BasicConsumeOptions,BasicQosOptions};
use solradar_common::{database,channel,PoolEvent,config};
use sqlx::Row;

#[tokio::main]async fn main()->Result<()>{
	solradar_common::init();
	let db=database().await?;
	let mq=channel().await?;

	mq.basic_qos(20,BasicQosOptions::default()).await?;
	
	let mut stream=mq.basic_consume("market.arbitrage","arbitrage-detector",BasicConsumeOptions::default(),Default::default()).await?;
	
	while let Some(d)=stream.next().await{
		let d=d?;
		let result:Result<()> = async{

			let evt:PoolEvent=serde_json::from_slice(&d.data)?;
			let rows = sqlx::query(
				    r#"
				    SELECT
				        pool_address,
				        quote_token_address,
				        CAST(price_usd AS CHAR) AS price,
				        CAST(liquidity_usd AS CHAR) AS liquidity
				    FROM solana_pools
				    WHERE token_address = ?
				      AND last_seen >= DATE_SUB(UTC_TIMESTAMP(), INTERVAL 3 MINUTE)
				      AND liquidity_usd >= ?
				      AND price_usd > 0
				    "#
				)
				.bind(&evt.token_address)
				.bind(config("ARBITRAGE_MIN_LIQUIDITY_USD", 20_000.0))
				.fetch_all(&db)
				.await?;

			for (i,a) in rows.iter().enumerate(){
				for b in rows.iter().skip(i+1){

			        let quote_a: String = a.get("quote_token_address");
			        let quote_b: String = b.get("quote_token_address");

			        if quote_a != quote_b {
			            continue;
			        }

			        let pa: f64 = a.get::<String, _>("price").parse().unwrap_or(0.0);
			        let pb: f64 = b.get::<String, _>("price") .parse().unwrap_or(0.0);

			        if pa <= 0.0 || pb <= 0.0 {
			            continue;
			        }

					let spread = solradar_market_models::spread_percent(pa,pb).unwrap_or(0.);

					if spread>=config("ARBITRAGE_MIN_SPREAD_PCT",0.5){

						let pool_a: String = a.get("pool_address");
					    let pool_b: String = b.get("pool_address");
					    let observed_at = evt.captured_at.naive_utc();

					    let reason = format!(
					        "price_a={pa:.12}; \
					         price_b={pb:.12}; \
					         spread={spread:.3}%; \
					         quote_token={quote_a}; \
					         executable=false"
					    );

					    let res = sqlx::query(
					        r#"
					        INSERT IGNORE INTO signals_arbitrage (
					            id,
					            token_address,
					            pool_address,
					            secondary_pool,
					            observed_at,
					            reason
					        )
					        VALUES (
					            UUID(),
					            'ARBITRAGE',
					            ?, ?, ?, ?, ?
					        )
					        "#
					    )
					    .bind(&evt.token_address)
					    .bind(&pool_a)
					    .bind(&pool_b)
					    .bind(observed_at)
					    .bind(&reason)
					    .execute(&db)
					    .await?;

					    if res.rows_affected() > 0 {
					        println!(
					            "ARBITRAGE SAVED {} vs {} \
					             price_a={pa:.12} \
					             price_b={pb:.12} \
					             spread={spread:.3}%",
					            pool_a,
					            pool_b
					        );
					    }
					}
				}
			}
			Ok(())
		}
		.await;

		match result{
			Ok(())=>d.ack(BasicAckOptions::default()).await?,Err(e)=>{
				eprintln!("arbitrage error: {e:#}");
				d.nack(lapin::options::BasicNackOptions{
					requeue:true,..Default::default()
				})
				.await?;
			}
		}
	}
	Ok(())
}

use anyhow::Result;
use futures_util::StreamExt;
use lapin::options::{BasicAckOptions,BasicNackOptions,BasicConsumeOptions,BasicQosOptions};
use solradar_common::{database,channel,PoolEvent,config};
use sqlx::{MySqlPool, Row};

async fn get_sol_price(db: &MySqlPool) -> Result<Option<f64>> {
    let row = sqlx::query(
        r#"
        SELECT CAST(price_usd AS CHAR) AS price
        FROM solana_pools
        WHERE token_address = 'So11111111111111111111111111111111111111112'
          AND quote_token_address = 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v'
          AND price_usd > 0
          AND last_seen >= DATE_SUB( UTC_TIMESTAMP(), INTERVAL 3 MINUTE )
        ORDER BY liquidity_usd DESC
        LIMIT 1
        "#
    )
    .fetch_optional(db)
    .await?;

    Ok(row
        .and_then(|r| r.try_get::<String, _>("price").ok())
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|p| p.is_finite() && *p > 0.0)
    )
}

#[tokio::main]async fn main()->Result<()>{
	solradar_common::init();
	let db=database().await?;
	let mq=channel().await?;
    let min_liquidity = config( "ARBITRAGE_MIN_LIQUIDITY_USD", 20_000.0);
    let min_spread = config( "ARBITRAGE_MIN_SPREAD_PCT", 0.5);
    let trade_amount: f64 = config("ARBITRAGE_TRADE_AMOUNT_USD", 1000.0_f64);
    let min_profit = config("ARBITRAGE_MIN_NET_PROFIT_USD", 1.0);

    if !trade_amount.is_finite() || trade_amount <= 0.0 {
        anyhow::bail!("Invalid operation value");
    }

	mq.basic_qos(20,BasicQosOptions::default()).await?;
	
	let mut stream=mq.basic_consume("market.arbitrage","arbitrage-detector",BasicConsumeOptions::default(),Default::default()).await?;
	
	while let Some(d)=stream.next().await{
		let d=d?;
		let result:Result<()> = async{

			let evt:PoolEvent=serde_json::from_slice(&d.data)?;

			// event validation
            let age = chrono::Utc::now()
                .signed_duration_since(evt.captured_at)
                .num_seconds();
            if age > 120 {
                println!(
                    "ARBITRAGE STALE EVENT token={} age={}s",
                    evt.token_address, age
                );
                return Ok(());
            }

            // pools and fees
			let rows = sqlx::query(
			    r#"
			    SELECT
			        pool_address,
                	dex_id,
			        quote_token_address,

			        CAST(price_usd AS CHAR) AS price,
			        CAST(liquidity_usd AS CHAR) AS liquidity,

                    CASE
                        WHEN fee_updated_at >=
                            DATE_SUB(
                                UTC_TIMESTAMP(),
                                INTERVAL 60 MINUTE
                            )
                        THEN CAST(fee_pct AS CHAR)
                        ELSE NULL
                    END AS dex_fee,

                    CASE
                        WHEN network_fee_updated_at >=
                            DATE_SUB(
                                UTC_TIMESTAMP(),
                                INTERVAL 10 MINUTE
                            )
                        THEN CAST(
                            network_fee_lamports AS CHAR
                        )
                        ELSE NULL
                    END AS network_fee

			    FROM solana_pools
			    WHERE token_address = ?
			      AND last_seen >= DATE_SUB(UTC_TIMESTAMP(), INTERVAL 3 MINUTE)
			      AND liquidity_usd >= ?
			      AND price_usd > 0
			    "#
			)
			.bind(&evt.token_address)
        	.bind(min_liquidity)
			.fetch_all(&db)
			.await?;

            if rows.len() < 2 {
                return Ok(());
            }

            // Conversion reference:
			// lamports -> SOL -> USD
            let sol_price = get_sol_price(&db).await?;

            // compare pools
			for (i,a) in rows.iter().enumerate(){
				for b in rows.iter().skip(i+1){

			        let quote_a: String = a.get("quote_token_address");
			        let quote_b: String = b.get("quote_token_address");
			        if quote_a != quote_b {
			            continue;
			        }

			        let pa: f64 = a.get::<String, _>("price").parse().unwrap_or(0.0);
			        let pb: f64 = b.get::<String, _>("price") .parse().unwrap_or(0.0);
                    if !pa.is_finite()
                        || !pb.is_finite()
                        || pa <= 0.0
                        || pb <= 0.0
                    {
                        continue;
                    }

					let buy_price = pa.min(pb);
					let sell_price = pa.max(pb);
					let spread = ((sell_price / buy_price) - 1.0) * 100.0;
                    if !spread.is_finite() || spread < min_spread
                    {
                        continue;
                    }

					let pool_a: String = a.get("pool_address");
				    let pool_b: String = b.get("pool_address");
				    let dex_a: String = a.get("dex_id");
                	let dex_b: String = b.get("dex_id");
				    let observed_at = evt.captured_at.naive_utc();

				    // identify buy and sell
					let (
                        buy_price,
                        sell_price,
                        buy_fee_raw,
                        sell_fee_raw,
                        buy_pool,
                        sell_pool,
                    ) = if pa < pb {
                        (
                            pa,
                            pb,
                            a.try_get::<Option<String>, _>( "dex_fee" )?,
                            b.try_get::<Option<String>, _>( "dex_fee" )?,
                            pool_a.as_str(),
                            pool_b.as_str(),
                        )
                    } else {
                        (
                            pb,
                            pa,
                            b.try_get::<Option<String>, _>( "dex_fee" )?,
                            a.try_get::<Option<String>, _>( "dex_fee" )?,
                            pool_b.as_str(),
                            pool_a.as_str(),
                        )
                    };

                    // Never treat a missing rate as zero
                    let buy_fee = buy_fee_raw.and_then(|v| v.parse::<f64>().ok());
					let sell_fee = sell_fee_raw.and_then(|v| v.parse::<f64>().ok());
					let (buy_fee, sell_fee) = match (buy_fee, sell_fee) {
					                            (Some(x), Some(y))
					                                if x.is_finite()
					                                    && y.is_finite()
					                                    && (0.0..100.0).contains(&x)
					                                    && (0.0..100.0).contains(&y) =>
							                            {
							                                (x, y)
							                            }
						                            _ => {
						                                continue;
						                            }
					                        };

					// network rate reference
                    let network_a = a.try_get::<Option<String>, _>( "network_fee" )?.and_then(|v| v.parse::<u64>().ok());
                    let network_b = b.try_get::<Option<String>, _>( "network_fee" )?.and_then(|v| v.parse::<u64>().ok());
                    let network_lamports = match (network_a, network_b) {
					                            (Some(x), Some(y)) => x.max(y),
					                            _ => continue,
					                        };
                    let sol_usd = match sol_price { Some(p) => p, None => continue, };
					let network_fee_usd = network_lamports as f64 / 1_000_000_000.0 * sol_usd;

					// calculate return after fees
                    let buy_fee_factor = 1.0 - buy_fee / 100.0;
                    let sell_fee_factor = 1.0 - sell_fee / 100.0;
                    let tokens_bought = (trade_amount / buy_price) * buy_fee_factor;
                    let amount_received = tokens_bought * sell_price * sell_fee_factor;
                    let dex_fees_usd = trade_amount * buy_fee / 100.0 + tokens_bought * sell_price * sell_fee / 100.0;
                    let estimated_profit = amount_received - trade_amount - network_fee_usd;
                    let estimated_profit_pct = estimated_profit / trade_amount * 100.0;
                    if !estimated_profit.is_finite() {
                        continue;
                    }

                    // filter profitability
                    if estimated_profit < min_profit {
                        continue;
                    }

                    // save signal
					let reason = format!(
                        "spread_bruto={spread:.6}%; \
                         dex_a={dex_a}; dex_b={dex_b}; \
                         buy_pool={buy_pool}; \
                         sell_pool={sell_pool}; \
                         buy_fee={buy_fee:.6}%; \
                         sell_fee={sell_fee:.6}%; \
                         dex_fees_usd={dex_fees_usd:.6}; \
                         network_reference_lamports={network_lamports}; \
                         network_reference_usd={network_fee_usd:.6}; \
                         trade_amount_usd={trade_amount:.2}; \
                         estimated_profit_usd={estimated_profit:.6}; \
                         estimated_profit_pct={estimated_profit_pct:.6}%; \
                         price_impact_included=false; \
                         executable=false"
                    );

				    let res = sqlx::query(
					    r#"
					    INSERT IGNORE INTO signals_arbitrage (
					        id,
					        token_address,
					        pool_address,
					        secondary_pool,
					        quote_token_address,
					        price_a,
					        price_b,
					        spread_pct,
					        observed_at,
					        reason
					    )
					    VALUES (
					        UUID(), ?, ?, ?, ?, ?, ?, ?, ?, ?
					    )
					    "#
					)
					.bind(&evt.token_address)
					.bind(&pool_a)
					.bind(&pool_b)
					.bind(&quote_a)
					.bind(format!("{pa:.18}"))
					.bind(format!("{pb:.18}"))
					.bind(format!("{spread:.8}"))
					.bind(observed_at)
					.bind(&reason)
					.execute(&db)
					.await?;

					if res.rows_affected() > 0 {
                        println!(
						    "ARBITRAGE SAVED \
						     token={} \
						     buy_pool={} \
						     sell_pool={} \
						     buy_price=${:.12} \
						     sell_price=${:.12} \
						     spread_bruto={:.4}% \
						     taxa_compra={:.4}% \
						     taxa_venda={:.4}% \
						     taxa_rede=${:.6} \
						     lucro_liquido=${:.4} \
						     arbitragem_liquida={:.4}%",
						    evt.token_address,
						    buy_pool,
						    sell_pool,
						    buy_price,
						    sell_price,
						    spread,
						    buy_fee,
						    sell_fee,
						    network_fee_usd,
						    estimated_profit,
						    estimated_profit_pct
						);
                    }
				}
			}
			Ok(())
		}
		.await;

        match result {
            Ok(()) => {
                d.ack(
                    BasicAckOptions::default()
                ).await?;
            }
            Err(e) => {
                eprintln!(
                    "arbitrage error: {e:#}"
                );

                d.nack(
                    BasicNackOptions {
                        requeue: true,
                        ..Default::default()
                    }
                ).await?;
            }
        }
	}
	Ok(())
}

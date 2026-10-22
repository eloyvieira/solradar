use anyhow::Result;
use solradar_common::{database,config};
use sqlx::Row;
use std::time::Duration;

#[tokio::main]async fn main()->Result<()>{
	solradar_common::init();let db=database().await?;

	loop{
		let rows=sqlx::query("SELECT id,pool_address,CAST(entry_price_usd AS CHAR) AS entry,CAST(peak_price_usd AS CHAR) AS peak,CAST(stop_price_usd AS CHAR) AS stop FROM positions WHERE status='OPEN' LIMIT 500").fetch_all(&db).await?;
		for row in rows{
			let id:String=row.get("id");
			let pool:String=row.get("pool_address");
			let price=sqlx::query("SELECT CAST(price_usd AS CHAR) AS price,last_seen FROM solana_pools WHERE pool_address=?").bind(&pool).fetch_optional(&db).await?;
			if let Some(p)=price{
				let observed:chrono::NaiveDateTime=p.get("last_seen");
				if (chrono::Utc::now().naive_utc()-observed).num_seconds()>60{
					continue
				}
				let current:f64=p.get::<String,_>("price").parse().unwrap_or(0.);
				let peak:f64=row.get::<String,_>("peak").parse().unwrap_or(0.);
				let stop:f64=row.get::<String,_>("stop").parse().unwrap_or(0.);
				if current<=0.{
					continue
				}
				let trailing=config("TRAILING_STOP_PCT",5_f64).clamp(0.1,95.);
				let Some((high,new_stop,should_exit))=solradar_market_models::trailing_stop(peak,current,stop,trailing) else {
					continue
				};
				if should_exit{

					sqlx::query("UPDATE positions SET status='EXIT_REQUESTED',peak_price_usd=?,stop_price_usd=? WHERE id=? AND status='OPEN'")
						.bind(format!("{high:.12}"))
						.bind(format!("{new_stop:.12}"))
						.bind(&id)
						.execute(&db)
						.await?;
		
					println!("SAÍDA NECESSÁRIA posição={id}, preço={current}, stop={new_stop}; sem swap real");
				
				}else{
					sqlx::query("UPDATE positions SET peak_price_usd=?,stop_price_usd=? WHERE id=? AND status='OPEN'")
						.bind(format!("{high:.12}"))
						.bind(format!("{new_stop:.12}"))
						.bind(&id)
						.execute(&db)
						.await?;
				}
			}
		}
		tokio::time::sleep(Duration::from_secs(config("POSITION_POLL_SECONDS",5_u64))).await;
	}
}

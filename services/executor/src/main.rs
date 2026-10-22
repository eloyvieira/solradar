use anyhow::Result;
use chrono::Utc;
use futures_util::StreamExt;
use lapin::options::{BasicAckOptions,BasicConsumeOptions,BasicQosOptions};
use solradar_common::{database,channel,Signal,LedgerEvent,publish,config};
use sqlx::Row;
use uuid::Uuid;

#[tokio::main]async fn main()->Result<()>{
	solradar_common::init();
	let db=database().await?;
	let mq=channel().await?;
	mq.basic_qos(10,BasicQosOptions::default()).await?;
	let mut stream=mq.basic_consume("signals.execute","executor",BasicConsumeOptions::default(),Default::default()).await?;
	
	while let Some(d)=stream.next().await{
		let d=d?;
		let work:Result<()> = async{

			let sig:Signal=serde_json::from_slice(&d.data)?;
			let row=sqlx::query("SELECT status,expires_at FROM token_security WHERE token_address=?").bind(&sig.token_address).fetch_optional(&db).await?;
			let safe=row.map(|r|{
				let status:String=r.get("status");
				let expiry:chrono::NaiveDateTime=r.get("expires_at");
				status=="APPROVED"&&expiry>Utc::now().naive_utc()
			})
			.unwrap_or(false);

			let age=(Utc::now()-sig.observed_at).num_seconds();
			let status=if !safe{"REJECTED_SECURITY"}else if age>config("MAX_SIGNAL_AGE_SECONDS",30_i64){"STALE_SIGNAL"}else{"SIMULATION_ONLY"};
			let id=Uuid::new_v4().to_string();

			sqlx::query("INSERT INTO trade_orders(id,signal_id,strategy,token_address,pool_address,status,mode,error_message) VALUES(?,?,?,?,?,?,?,?)")
					.bind(&id)
					.bind(&sig.id)
					.bind(&sig.strategy)
					.bind(&sig.token_address)
					.bind(&sig.pool_address)
					.bind(status)
					.bind("PAPER")
					.bind("Execução real não implementada; checagem de liquidez/quote on-chain pendente")
					.execute(&db).await?;

			sqlx::query("UPDATE trade_signals SET status=? WHERE id=?").bind(status).bind(&sig.id).execute(&db).await?;

			publish(&mq,"ledger.events",&LedgerEvent{
				order_id:id,event_type:"ORDER_RECORDED".into()
			}).await?;
			
			println!("{} {} {}",sig.strategy,sig.token_address,status);
			Ok(())

		}.await;
	match work{Ok(())=>d.ack(BasicAckOptions::default()).await?,Err(e)=>{eprintln!("executor error: {e:#}");d.nack(lapin::options::BasicNackOptions{requeue:true,..Default::default()}).await?;tokio::time::sleep(std::time::Duration::from_secs(2)).await;}}
	}Ok(())
}

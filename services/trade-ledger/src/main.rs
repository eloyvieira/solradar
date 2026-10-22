use anyhow::Result;
use futures_util::StreamExt;
use lapin::options::{BasicAckOptions,BasicConsumeOptions};
use solradar_common::{database,channel,LedgerEvent};
use sqlx::Row;

#[tokio::main]async fn main()->Result<()>{

	solradar_common::init();
	let db=database().await?;
	let mq=channel().await?;
	let mut stream=mq.basic_consume("ledger.events","trade-ledger",BasicConsumeOptions::default(),Default::default()).await?;
	while let Some(d)=stream.next().await{
		let d=d?;
		let work:Result<()> = async{
			let ev:LedgerEvent=serde_json::from_slice(&d.data)?;
			let order=sqlx::query("SELECT status FROM trade_orders WHERE id=?")
				.bind(&ev.order_id)
				.fetch_optional(&db)
				.await?;
			if let Some(r)=order{
				let status:String=r.get("status");
				sqlx::query("INSERT INTO trade_results(order_id,status,notes) VALUES(?,?,?) ON DUPLICATE KEY UPDATE status=VALUES(status),notes=VALUES(notes),reconciled_at=UTC_TIMESTAMP(6)")
					.bind(&ev.order_id)
					.bind(&status)
					.bind("Sem PnL: nenhuma transação on-chain executada")
					.execute(&db)
					.await?;
			}
			Ok(())
		}.await;
		
		match work{
			Ok(())=>d.ack(BasicAckOptions::default()).await?,Err(e)=>{
				eprintln!("ledger error: {e:#}");
				d.nack(lapin::options::BasicNackOptions{
					requeue:true,..Default::default()
				})
				.await?;
			}
		}
	}
	Ok(())
}

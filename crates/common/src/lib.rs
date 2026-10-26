use anyhow::{Context,Result};
use chrono::{DateTime,Utc};
use lapin::{options::*,types::FieldTable,BasicProperties,Channel,Connection,ConnectionProperties};
use rust_decimal::Decimal;
use serde::{Deserialize,Serialize};
use sqlx::{mysql::MySqlPoolOptions,MySqlPool};
use std::{env,str::FromStr};

pub fn init() {
	dotenvy::dotenv().ok();
	tracing_subscriber_placeholder();
}
fn tracing_subscriber_placeholder() {}
pub fn config<T: FromStr>(key:&str,default:T)->T {
	env::var(key).ok().and_then(|x|x.parse().ok()).unwrap_or(default)
}
pub async fn database()->Result<MySqlPool>{
	let url=env::var("DATABASE_URL").context("DATABASE_URL não configurada")?;
	Ok(MySqlPoolOptions::new().max_connections(8).connect(&url).await?)
}
pub async fn channel()->Result<Channel>{
	let url=env::var("AMQP_URL").unwrap_or("amqp://guest:guest@localhost:5672/%2f".into());
	let conn=Connection::connect(&url,ConnectionProperties::default()).await?;
	let c=conn.create_channel().await?;
	for q in [
	    "market.arbitrage",
	    "market.pump",
	    "signals.execute",
	    "signals.arbitrage.execute",
	    "positions.monitor",
	    "ledger.events",
	] {
		c.queue_declare(q,
						QueueDeclareOptions{
							durable:true,..Default::default()
						},
						FieldTable::default()
					).await?;
	}
	Ok(c)
}
pub async fn publish<T:Serialize>(channel:&Channel,queue:&str,event:&T)->Result<()> {
	let payload=serde_json::to_vec(event)?;
	channel.basic_publish("",
						queue,BasicPublishOptions::default(),
						&payload,
						BasicProperties::default().with_delivery_mode(2)
					).await?.await?;
	Ok(())
}
pub fn dec(v:&str)->Decimal{
	Decimal::from_str(v).unwrap_or(Decimal::ZERO)
}

#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct PoolEvent {
	pub pool_address:String,
	pub token_address:String,
	pub captured_at:DateTime<Utc>
}

#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct Signal {
	pub id:String,pub strategy:String,pub pool_address:String,pub secondary_pool:Option<String>,pub token_address:String,pub observed_at:DateTime<Utc>,pub reason:String
}

#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct LedgerEvent {
	pub order_id:String,pub event_type:String
}

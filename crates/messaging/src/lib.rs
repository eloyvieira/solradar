use anyhow::Result;
use lapin::{Channel,BasicProperties,options::BasicPublishOptions};
use serde::Serialize;
pub async fn publish_json<T:Serialize>(ch:&Channel,queue:&str,payload:&T)->Result<()> {
 let bytes=serde_json::to_vec(payload)?;
 ch.basic_publish("",queue,BasicPublishOptions::default(),&bytes,BasicProperties::default().with_delivery_mode(2)).await?.await?;
 Ok(())
}

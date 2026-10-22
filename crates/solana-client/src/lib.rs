use anyhow::{Context,Result};
use serde_json::{json,Value};
#[derive(Clone)] pub struct Rpc { client:reqwest::Client, url:String }
impl Rpc {
 pub fn new(url:String)->Self {Self{client:reqwest::Client::new(),url}}
 pub async fn account_info(&self,mint:&str)->Result<Value>{
  let payload=json!({"jsonrpc":"2.0","id":1,"method":"getAccountInfo","params":[mint,{"encoding":"jsonParsed"}]});
  let r=self.client.post(&self.url).json(&payload).send().await?.error_for_status()?;
  let v:Value=r.json().await?;
  if !v["error"].is_null(){anyhow::bail!("RPC: {}",v["error"]);}
  Ok(v["result"]["value"].clone())
 }
 pub async fn health(&self)->Result<String>{
  let r=self.client.post(&self.url).json(&json!({"jsonrpc":"2.0","id":1,"method":"getHealth"})).send().await?.error_for_status()?;
  let v:Value=r.json().await.context("Resposta RPC inválida")?;
  Ok(v["result"].as_str().unwrap_or("unknown").to_string())
 }
}

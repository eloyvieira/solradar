use anyhow::{Context,Result};
use chrono::{TimeZone,Utc};
use serde_json::Value;
use solradar_common::{config,database,channel,publish,PoolEvent};
use sqlx::{MySqlPool,Row};
use std::{collections::HashMap,time::Duration};
use tokio::time::{interval,sleep,MissedTickBehavior};

fn number(v:&Value)->f64{match v{Value::Number(n)=>n.as_f64().unwrap_or(0.),Value::String(s)=>s.parse().unwrap_or(0.),_=>0.}}
fn numeric(v:f64)->String{format!("{:.12}",if v.is_finite(){v}else{0.})}
fn count(v:&Value)->u32{number(v).clamp(0.,u32::MAX as f64) as u32}

async fn fetch_page(client:&reqwest::Client,page:usize)->Result<Value>{
  let url=format!("https://api.geckoterminal.com/api/v2/networks/solana/pools?page={page}&sort=h24_volume_usd_desc&include=base_token,quote_token,dex");
  let r=client.get(url).header("Accept","application/json;version=20230203").send().await?;
  let status=r.status();
  if !status.is_success(){
    anyhow::bail!("GeckoTerminal HTTP {status}: {}",r.text().await.unwrap_or_default())
  }
  Ok(r.json().await?)
}

async fn save(db:&MySqlPool,pool:&Value,tokens:&HashMap<String,(String,String)>,min_vol:f64,min_liq:f64)->Result<Option<PoolEvent>>{
  let a=&pool["attributes"];let address=a["address"].as_str().unwrap_or("");
  let vol=number(&a["volume_usd"]["h24"]);let liq=number(&a["reserve_in_usd"]);

  if address.is_empty()||vol<min_vol||liq<min_liq{return Ok(None)}

  let base=pool["relationships"]["base_token"]["data"]["id"].as_str().unwrap_or("");
  let quote=pool["relationships"]["quote_token"]["data"]["id"].as_str().unwrap_or("");
  let dex=pool["relationships"]["dex"]["data"]["id"].as_str().unwrap_or("");
  let token=base.strip_prefix("solana_").unwrap_or(base);

  if token.is_empty(){return Ok(None)}
  
  let quote=quote.strip_prefix("solana_").unwrap_or(quote);
  let (symbol,name)=tokens.get(base).cloned().unwrap_or_default();
  let price=number(&a["base_token_price_usd"]);let m5=number(&a["volume_usd"]["m5"]);let h1=number(&a["volume_usd"]["h1"]);
  let buys=count(&a["transactions"]["m5"]["buys"]);let sells=count(&a["transactions"]["m5"]["sells"]);
  let change=number(&a["price_change_percentage"]["m5"]);
  let now=Utc::now();let minute=Utc.timestamp_opt(now.timestamp()/60*60,0).single().context("minuto inválido")?;
  let mut tx=db.begin().await?;
  
  sqlx::query("INSERT INTO solana_pools (pool_address,token_address,quote_token_address,dex_id,token_symbol,token_name,pair_name,price_usd,volume_m5_usd,volume_h1_usd,volume_h24_usd,liquidity_usd,buys_m5,sells_m5,change_m5_pct,first_seen,last_seen) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE token_address=VALUES(token_address),quote_token_address=VALUES(quote_token_address),dex_id=VALUES(dex_id),token_symbol=VALUES(token_symbol),token_name=VALUES(token_name),pair_name=VALUES(pair_name),price_usd=VALUES(price_usd),volume_m5_usd=VALUES(volume_m5_usd),volume_h1_usd=VALUES(volume_h1_usd),volume_h24_usd=VALUES(volume_h24_usd),liquidity_usd=VALUES(liquidity_usd),buys_m5=VALUES(buys_m5),sells_m5=VALUES(sells_m5),change_m5_pct=VALUES(change_m5_pct),last_seen=VALUES(last_seen)")
    .bind(address).bind(token).bind(quote).bind(dex).bind(&symbol).bind(&name).bind(a["name"].as_str().unwrap_or(""))
    .bind(numeric(price)).bind(numeric(m5)).bind(numeric(h1)).bind(numeric(vol)).bind(numeric(liq))
    .bind(buys).bind(sells).bind(numeric(change)).bind(now.naive_utc()).bind(now.naive_utc()).execute(&mut *tx).await?;
  
  let old=sqlx::query("SELECT CAST(volume_m5_usd AS CHAR) AS v,TIMESTAMPDIFF(SECOND,captured_at,?) AS age_s FROM solana_pool_snapshots WHERE pool_address=? AND captured_at <= DATE_SUB(?,INTERVAL 5 MINUTE) ORDER BY captured_at DESC LIMIT 1")
    .bind(minute.naive_utc()).bind(address).bind(minute.naive_utc()).fetch_optional(&mut *tx).await?;
  
  let acceleration=old.and_then(|r|{let age:i64=r.try_get("age_s").ok()?;let old:f64=r.try_get::<String,_>("v").ok()?.parse().ok()?;solradar_market_models::volume_acceleration(m5,old,age)});
  
  sqlx::query("INSERT INTO solana_pool_snapshots (pool_address,captured_at,price_usd,volume_m5_usd,volume_h1_usd,volume_h24_usd,liquidity_usd,buys_m5,sells_m5,change_m5_pct,volume_acceleration_5m,pump_candidate) VALUES (?,?,?,?,?,?,?,?,?,?,?,FALSE) ON DUPLICATE KEY UPDATE price_usd=VALUES(price_usd),volume_m5_usd=VALUES(volume_m5_usd),volume_h1_usd=VALUES(volume_h1_usd),volume_h24_usd=VALUES(volume_h24_usd),liquidity_usd=VALUES(liquidity_usd),buys_m5=VALUES(buys_m5),sells_m5=VALUES(sells_m5),change_m5_pct=VALUES(change_m5_pct),volume_acceleration_5m=VALUES(volume_acceleration_5m)")
    .bind(address).bind(minute.naive_utc()).bind(numeric(price)).bind(numeric(m5)).bind(numeric(h1)).bind(numeric(vol)).bind(numeric(liq)).bind(buys).bind(sells).bind(numeric(change)).bind(acceleration.map(numeric)).execute(&mut *tx).await?;
  tx.commit().await?;
  Ok(Some(PoolEvent{pool_address:address.into(),token_address:token.into(),captured_at:minute}))
}

#[tokio::main]async fn main()->Result<()>{
  solradar_common::init();let db=database().await?;let mq=channel().await?;

  let min_vol=config("MIN_VOLUME_H24_USD",300_000.0);let min_liq=config("MIN_LIQUIDITY_USD",0.0);
  let pages=config("MAX_PAGES",3_usize).clamp(1,10);let spacing=config("REQUEST_SPACING_SECONDS",7_u64).max(6);
  let seconds=config("SCAN_INTERVAL_SECONDS",60_u64).max(60);
  let client=reqwest::Client::builder().timeout(Duration::from_secs(25)).user_agent("solradar-collector/0.1").build()?;
  let mut timer=interval(Duration::from_secs(seconds));timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
  
  loop{
    timer.tick().await;let mut saved=0;

    for page in 1..=pages {
      if page>1{
        sleep(Duration::from_secs(spacing)).await;
      }
      match fetch_page(&client,page).await{
        Ok(json)=>{
          let mut tokens=HashMap::new();if let Some(included)=json["included"].as_array(){
            for t in included{if let Some(id)=t["id"].as_str(){tokens.insert(id.into(),(t["attributes"]["symbol"].as_str().unwrap_or("").into(),t["attributes"]["name"].as_str().unwrap_or("").into()));}}
          }
          if let Some(pools)=json["data"].as_array(){
            for pool in pools{
              match save(&db,pool,&tokens,min_vol,min_liq).await{
                Ok(Some(event))=>{
                  saved+=1;
                  for q in ["market.arbitrage","market.pump"]{
                    if let Err(e)=publish(&mq,q,&event).await{
                      eprintln!("Evento {q} não publicado: {e:#}");
                    }
                  }
                },
                Ok(None)=>{},Err(e)=>eprintln!("Erro pool: {e:#}")
              } 
            }
          }
        },
        Err(e)=>{
          eprintln!("Erro página {page}: {e:#}");if e.to_string().contains("429"){sleep(Duration::from_secs(65)).await;break}
        }
      }
    }
    println!("{} UTC | pools persistidas: {saved}",Utc::now());
  }
}

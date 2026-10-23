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
    let a=&pool["attributes"];
    let address=a["address"].as_str().unwrap_or("");
    let vol=number(&a["volume_usd"]["h24"]);
    let liq=number(&a["reserve_in_usd"]);

    if address.is_empty()||vol<min_vol||liq<min_liq{return Ok(None)}

    let base=pool["relationships"]["base_token"]["data"]["id"].as_str().unwrap_or("");
    let quote=pool["relationships"]["quote_token"]["data"]["id"].as_str().unwrap_or("");
    let dex=pool["relationships"]["dex"]["data"]["id"].as_str().unwrap_or("");
    let token=base.strip_prefix("solana_").unwrap_or(base);

    if token.is_empty(){return Ok(None)}

    let quote=quote.strip_prefix("solana_").unwrap_or(quote);
    let (symbol,name)=tokens.get(base).cloned().unwrap_or_default();
    let pool_created_at = a["pool_created_at"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.naive_utc());
    let optional_decimal = |v: &Value| -> Option<String> {
        let n = match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.parse::<f64>().ok(),
            _ => None,
        }?;
        if n.is_finite() {
            Some(numeric(n))
        } else {
            None
        }
    };
    let optional_count = |v: &Value| -> Option<u32> {
        match v {
            Value::Number(n) => n
                .as_u64()
                .and_then(|n| u32::try_from(n).ok()),
            Value::String(s) => s.parse::<u32>().ok(),
            _ => None,
        }
    };
    let price=number(&a["base_token_price_usd"]);
    let m5=number(&a["volume_usd"]["m5"]);
    let h1=number(&a["volume_usd"]["h1"]);
    let change=number(&a["price_change_percentage"]["m5"]);
    
    let volume_m15 = optional_decimal(&a["volume_usd"]["m15"]);
    let volume_m30 = optional_decimal(&a["volume_usd"]["m30"]);
    let volume_h6 = optional_decimal(&a["volume_usd"]["h6"]);
    let change_m15 = optional_decimal(&a["price_change_percentage"]["m15"]);
    let change_m30 = optional_decimal(&a["price_change_percentage"]["m30"]);
    let change_h1 = optional_decimal(&a["price_change_percentage"]["h1"]);
    let change_h6 = optional_decimal(&a["price_change_percentage"]["h6"]);
    let change_h24 = optional_decimal(&a["price_change_percentage"]["h24"]);

    let buys=count(&a["transactions"]["m5"]["buys"]);
    let sells=count(&a["transactions"]["m5"]["sells"]);
    let buyers_m5 = optional_count(&a["transactions"]["m5"]["buyers"]);
    let sellers_m5 = optional_count(&a["transactions"]["m5"]["sellers"]);

    let buys_m15 = optional_count(&a["transactions"]["m15"]["buys"]);
    let sells_m15 = optional_count(&a["transactions"]["m15"]["sells"]);
    let buyers_m15 = optional_count(&a["transactions"]["m15"]["buyers"]);
    let sellers_m15 = optional_count(&a["transactions"]["m15"]["sellers"]);

    let buys_m30 = optional_count(&a["transactions"]["m30"]["buys"]);
    let sells_m30 = optional_count(&a["transactions"]["m30"]["sells"]);
    let buyers_m30 = optional_count(&a["transactions"]["m30"]["buyers"]);
    let sellers_m30 = optional_count(&a["transactions"]["m30"]["sellers"]);

    let buys_h1 = optional_count(&a["transactions"]["h1"]["buys"]);
    let sells_h1 = optional_count(&a["transactions"]["h1"]["sells"]);
    let buyers_h1 = optional_count(&a["transactions"]["h1"]["buyers"]);
    let sellers_h1 = optional_count(&a["transactions"]["h1"]["sellers"]);

    let buys_h24 = optional_count(&a["transactions"]["h24"]["buys"]);
    let sells_h24 = optional_count(&a["transactions"]["h24"]["sells"]);
    let buyers_h24 = optional_count(&a["transactions"]["h24"]["buyers"]);
    let sellers_h24 = optional_count(&a["transactions"]["h24"]["sellers"]);

    let fdv = optional_decimal(&a["fdv_usd"]);
    let market_cap = optional_decimal(&a["market_cap_usd"]);

    let now=Utc::now();
    let minute=Utc.timestamp_opt(now.timestamp()/60*60,0).single().context("minuto inválido")?;
    let mut tx=db.begin().await?;

    sqlx::query(
        r#"
        INSERT INTO solana_pools (
            pool_address,
            token_address,
            quote_token_address,
            dex_id,
            token_symbol,
            token_name,
            pair_name,
            price_usd,
            volume_m5_usd,
            volume_h1_usd,
            volume_h24_usd,
            liquidity_usd,
            buys_m5,
            sells_m5,
            change_m5_pct,
            pool_created_at,
            first_seen,
            last_seen
        )
        VALUES (
            ?, ?, ?, ?, ?, ?, ?, ?, ?,
            ?, ?, ?, ?, ?, ?, ?, ?, ?
        )
        ON DUPLICATE KEY UPDATE
            token_address       = VALUES(token_address),
            quote_token_address = VALUES(quote_token_address),
            dex_id              = VALUES(dex_id),
            token_symbol        = VALUES(token_symbol),
            token_name          = VALUES(token_name),
            pair_name           = VALUES(pair_name),
            price_usd           = VALUES(price_usd),
            volume_m5_usd       = VALUES(volume_m5_usd),
            volume_h1_usd       = VALUES(volume_h1_usd),
            volume_h24_usd      = VALUES(volume_h24_usd),
            liquidity_usd       = VALUES(liquidity_usd),
            buys_m5             = VALUES(buys_m5),
            sells_m5            = VALUES(sells_m5),
            change_m5_pct       = VALUES(change_m5_pct),
            pool_created_at     = COALESCE(
                VALUES(pool_created_at),
                pool_created_at
            ),
            last_seen           = VALUES(last_seen)
        "#
        )
        .bind(address)
        .bind(token)
        .bind(quote)
        .bind(dex)
        .bind(&symbol)
        .bind(&name)
        .bind(a["name"].as_str().unwrap_or(""))
        .bind(numeric(price))
        .bind(numeric(m5))
        .bind(numeric(h1))
        .bind(numeric(vol))
        .bind(numeric(liq))
        .bind(buys)
        .bind(sells)
        .bind(numeric(change))
        .bind(pool_created_at)
        .bind(now.naive_utc())
        .bind(now.naive_utc())
        .execute(&mut *tx)
        .await?;
  
    let old = sqlx::query(
        r#"
        SELECT
            CAST(volume_m5_usd AS CHAR) AS v,
            TIMESTAMPDIFF(
                SECOND,
                captured_at,
                ?
            ) AS age_s
        FROM
            solana_pool_snapshots
        WHERE
            pool_address = ?
            AND captured_at <= DATE_SUB(?, INTERVAL 5 MINUTE)
        ORDER BY
            captured_at DESC
        LIMIT 1
        "#
        )
        .bind(minute.naive_utc())
        .bind(address)
        .bind(minute.naive_utc())
        .fetch_optional(&mut *tx)
        .await?;

    let acceleration=old.and_then(|r|{
        let age:i64=r.try_get("age_s").ok()?;
        let previous_volume: f64 = r
            .try_get::<String, _>("v")
            .ok()?
            .parse()
            .ok()?;
        solradar_market_models::volume_acceleration(m5,previous_volume,age)
    });


    sqlx::query(
        r#"
        INSERT INTO solana_pool_snapshots (
            pool_address,
            captured_at,
            price_usd,
            volume_m5_usd,
            volume_h1_usd,
            volume_h24_usd,
            liquidity_usd,
            buys_m5,
            sells_m5,
            change_m5_pct,
            volume_acceleration_5m,

            volume_m15_usd,
            volume_m30_usd,
            volume_h6_usd,

            change_m15_pct,
            change_m30_pct,
            change_h1_pct,
            change_h6_pct,
            change_h24_pct,

            buys_m15,
            sells_m15,
            buys_m30,
            sells_m30,
            buys_h1,
            sells_h1,
            buys_h24,
            sells_h24,

            buyers_m5,
            sellers_m5,
            buyers_m15,
            sellers_m15,
            buyers_m30,
            sellers_m30,
            buyers_h1,
            sellers_h1,
            buyers_h24,
            sellers_h24,

            fdv_usd,
            market_cap_usd,
            pump_candidate
        )
        VALUES (
            ?, ?, ?, ?, ?, ?,
            ?, ?, ?, ?, ?,

            ?, ?, ?,

            ?, ?, ?, ?, ?,

            ?, ?, ?, ?, ?, ?, ?, ?,

            ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,

            ?, ?,
            FALSE
        )
        ON DUPLICATE KEY UPDATE
            price_usd              = VALUES(price_usd),
            volume_m5_usd          = VALUES(volume_m5_usd),
            volume_h1_usd          = VALUES(volume_h1_usd),
            volume_h24_usd         = VALUES(volume_h24_usd),
            liquidity_usd          = VALUES(liquidity_usd),
            buys_m5               = VALUES(buys_m5),
            sells_m5              = VALUES(sells_m5),
            change_m5_pct         = VALUES(change_m5_pct),
            volume_acceleration_5m = VALUES(volume_acceleration_5m),

            volume_m15_usd         = VALUES(volume_m15_usd),
            volume_m30_usd         = VALUES(volume_m30_usd),
            volume_h6_usd          = VALUES(volume_h6_usd),

            change_m15_pct         = VALUES(change_m15_pct),
            change_m30_pct         = VALUES(change_m30_pct),
            change_h1_pct          = VALUES(change_h1_pct),
            change_h6_pct          = VALUES(change_h6_pct),
            change_h24_pct         = VALUES(change_h24_pct),

            buys_m15              = VALUES(buys_m15),
            sells_m15             = VALUES(sells_m15),
            buys_m30              = VALUES(buys_m30),
            sells_m30             = VALUES(sells_m30),
            buys_h1               = VALUES(buys_h1),
            sells_h1              = VALUES(sells_h1),
            buys_h24              = VALUES(buys_h24),
            sells_h24             = VALUES(sells_h24),

            buyers_m5             = VALUES(buyers_m5),
            sellers_m5            = VALUES(sellers_m5),
            buyers_m15            = VALUES(buyers_m15),
            sellers_m15           = VALUES(sellers_m15),
            buyers_m30            = VALUES(buyers_m30),
            sellers_m30           = VALUES(sellers_m30),
            buyers_h1             = VALUES(buyers_h1),
            sellers_h1            = VALUES(sellers_h1),
            buyers_h24            = VALUES(buyers_h24),
            sellers_h24           = VALUES(sellers_h24),

            fdv_usd               = VALUES(fdv_usd),
            market_cap_usd        = VALUES(market_cap_usd)
        "#
        )
        .bind(address)
        .bind(minute.naive_utc())
        .bind(numeric(price))
        .bind(numeric(m5))
        .bind(numeric(h1))
        .bind(numeric(vol))
        .bind(numeric(liq))
        .bind(buys)
        .bind(sells)
        .bind(numeric(change))
        .bind(acceleration.map(numeric))
        .bind(volume_m15)
        .bind(volume_m30)
        .bind(volume_h6)
        .bind(change_m15)
        .bind(change_m30)
        .bind(change_h1)
        .bind(change_h6)
        .bind(change_h24)
        .bind(buys_m15)
        .bind(sells_m15)
        .bind(buys_m30)
        .bind(sells_m30)
        .bind(buys_h1)
        .bind(sells_h1)
        .bind(buys_h24)
        .bind(sells_h24)
        .bind(buyers_m5)
        .bind(sellers_m5)
        .bind(buyers_m15)
        .bind(sellers_m15)
        .bind(buyers_m30)
        .bind(sellers_m30)
        .bind(buyers_h1)
        .bind(sellers_h1)
        .bind(buyers_h24)
        .bind(sellers_h24)
        .bind(fdv)
        .bind(market_cap)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    
    Ok(Some(PoolEvent{
        pool_address:address.into(),token_address:token.into(),captured_at:minute
    }))
}

#[tokio::main]async fn main()->Result<()>{
    solradar_common::init();let db=database().await?;
    let mq=channel().await?;

    let min_vol=config("MIN_VOLUME_H24_USD",300_000.0);
    let min_liq=config("MIN_LIQUIDITY_USD",0.0);
    let pages=config("MAX_PAGES",3_usize).clamp(1,10);
    let spacing=config("REQUEST_SPACING_SECONDS",7_u64).max(6);
    let seconds=config("SCAN_INTERVAL_SECONDS",60_u64).max(60);
    let client=reqwest::Client::builder().timeout(Duration::from_secs(25)).user_agent("solradar-collector/0.1").build()?;
    let mut timer=interval(Duration::from_secs(seconds));
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop{
        timer.tick().await;
        let mut saved=0;

        for page in 1..=pages {
            if page>1{
                sleep(Duration::from_secs(spacing)).await;
            }
            match fetch_page(&client,page).await{
                Ok(json)=>{
                    let mut tokens=HashMap::new();
                    if let Some(included)=json["included"].as_array(){
                        for t in included{
                            if let Some(id)=t["id"].as_str(){
                                tokens.insert(id.into(),(t["attributes"]["symbol"].as_str().unwrap_or("").into(),t["attributes"]["name"].as_str().unwrap_or("").into()));
                            }
                        }
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

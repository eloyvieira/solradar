use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::Value;
use sqlx::{mysql::MySqlPoolOptions, MySqlPool, Row};
use std::{collections::HashMap, env, time::Duration};
use tokio::time::{interval, sleep, MissedTickBehavior};

fn number(v: &Value) -> f64 {
    match v { Value::Number(n) => n.as_f64().unwrap_or(0.0), Value::String(s) => s.parse().unwrap_or(0.0), _ => 0.0 }
}
fn count(v: &Value) -> u32 { number(v).max(0.0).min(u32::MAX as f64) as u32 }
fn decimal(v: f64) -> String { format!("{:.12}", if v.is_finite() {v} else {0.0}) }
fn optional_string(v: &Value) -> Option<String> { v.as_str().map(str::to_owned) }
fn env_num<T: std::str::FromStr>(key: &str, fallback: T) -> T { env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(fallback) }

async fn fetch_page(client: &reqwest::Client, page: usize) -> Result<Value> {
    let url = format!("https://api.geckoterminal.com/api/v2/networks/solana/pools?page={page}&sort=h24_volume_usd_desc&include=base_token");
    let res = client.get(&url).header("Accept", "application/json;version=20230203").send().await.context("Erro HTTP GeckoTerminal")?;
    let status = res.status();
    if !status.is_success() { anyhow::bail!("GeckoTerminal HTTP {}: {}", status, res.text().await.unwrap_or_default()); }
    Ok(res.json().await.context("JSON inválido")?)
}

async fn save_pool(db: &MySqlPool, pool: &Value, tokens: &HashMap<String, (String, String)>, min_volume: f64, min_liquidity: f64) -> Result<bool> {
    let a = &pool["attributes"];
    let address = a["address"].as_str().unwrap_or_default();
    let volume24 = number(&a["volume_usd"]["h24"]);
    let liq = number(&a["reserve_in_usd"]);
    if address.is_empty() || volume24 < min_volume || liq < min_liquidity { return Ok(false) }
    let token_id = pool["relationships"]["base_token"]["data"]["id"].as_str().unwrap_or_default();
    let token_address = token_id.strip_prefix("solana_").unwrap_or(token_id);
    let (symbol, name) = tokens.get(token_id).cloned().unwrap_or_default();
    let price = number(&a["base_token_price_usd"]);
    let vm5 = number(&a["volume_usd"]["m5"]);
    let vh1 = number(&a["volume_usd"]["h1"]);
    let buys = count(&a["transactions"]["m5"]["buys"]);
    let sells = count(&a["transactions"]["m5"]["sells"]);
    let change = number(&a["price_change_percentage"]["m5"]);
    let timestamp = Utc::now().naive_utc();
    // Arredondamento para minuto: evita duplicação dentro da mesma janela.
    let minute = chrono::DateTime::from_timestamp(
        timestamp.and_utc().timestamp() / 60 * 60,
        0
    ).unwrap().naive_utc();
    let mut tx = db.begin().await?;
    sqlx::query("INSERT INTO solana_pools (pool_address, token_address, token_symbol, token_name, pair_name, price_usd, volume_m5_usd, volume_h1_usd, volume_h24_usd, liquidity_usd, buys_m5, sells_m5, change_m5_pct, first_seen, last_seen) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON DUPLICATE KEY UPDATE token_address=VALUES(token_address), token_symbol=VALUES(token_symbol), token_name=VALUES(token_name), pair_name=VALUES(pair_name), price_usd=VALUES(price_usd), volume_m5_usd=VALUES(volume_m5_usd), volume_h1_usd=VALUES(volume_h1_usd), volume_h24_usd=VALUES(volume_h24_usd), liquidity_usd=VALUES(liquidity_usd), buys_m5=VALUES(buys_m5), sells_m5=VALUES(sells_m5), change_m5_pct=VALUES(change_m5_pct), last_seen=VALUES(last_seen)")
        .bind(address).bind(if token_address.is_empty() {None} else {Some(token_address)}).bind(if symbol.is_empty(){None}else{Some(symbol.as_str())}).bind(if name.is_empty(){None}else{Some(name.as_str())})
        .bind(optional_string(&a["name"])).bind(decimal(price)).bind(decimal(vm5)).bind(decimal(vh1)).bind(decimal(volume24)).bind(decimal(liq))
        .bind(buys).bind(sells).bind(decimal(change)).bind(timestamp).bind(timestamp).execute(&mut *tx).await?;

    // Aceleração = volume de 5m atual / volume de 5m observado ~5 minutos atrás.
    // Não usar delta entre duas janelas móveis de volume 24h como volume negociado.
    let past = sqlx::query("SELECT CAST(volume_m5_usd AS CHAR) AS v FROM solana_pool_snapshots WHERE pool_address=? AND captured_at <= DATE_SUB(?, INTERVAL 5 MINUTE) ORDER BY captured_at DESC LIMIT 1")
        .bind(address).bind(minute).fetch_optional(&mut *tx).await?;
    let past_v = past.and_then(|r| r.try_get::<Option<String>, _>("v").ok().flatten()).and_then(|v| v.parse::<f64>().ok());
    let acceleration = past_v.filter(|v| *v > 0.0).map(|v| vm5 / v);
    // Regra exploratória, não estratégia de compra: volume 5m pelo menos 2x,
    // preço 5m positivo >= 2%, pressão compradora >= 60% e liquidez >= 50k.
    let total = buys + sells;
    let buy_ratio = if total > 0 { buys as f64 / total as f64 } else { 0.0 };
    let is_pump = acceleration.unwrap_or(0.0) >= 2.0 && change >= 2.0 && buy_ratio >= 0.60 && liq >= 50_000.0;
    sqlx::query("INSERT INTO solana_pool_snapshots (pool_address, captured_at, price_usd, volume_m5_usd, volume_h1_usd, volume_h24_usd, liquidity_usd, buys_m5, sells_m5, change_m5_pct, volume_acceleration_5m, pump_candidate) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON DUPLICATE KEY UPDATE price_usd=VALUES(price_usd), volume_m5_usd=VALUES(volume_m5_usd), volume_h1_usd=VALUES(volume_h1_usd), volume_h24_usd=VALUES(volume_h24_usd), liquidity_usd=VALUES(liquidity_usd), buys_m5=VALUES(buys_m5), sells_m5=VALUES(sells_m5), change_m5_pct=VALUES(change_m5_pct), volume_acceleration_5m=VALUES(volume_acceleration_5m), pump_candidate=VALUES(pump_candidate)")
        .bind(address).bind(minute).bind(decimal(price)).bind(decimal(vm5)).bind(decimal(vh1)).bind(decimal(volume24)).bind(decimal(liq)).bind(buys).bind(sells).bind(decimal(change)).bind(acceleration.map(decimal)).bind(is_pump).execute(&mut *tx).await?;
    tx.commit().await?;
    if is_pump { println!("POSSÍVEL PUMP | {address} | {} | volume 5m {:.0} | aceleração {:.2}x", symbol, vm5, acceleration.unwrap_or(0.0)); }
    Ok(true)
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let database = env::var("DATABASE_URL").context("Configure DATABASE_URL no .env")?;
    let db = MySqlPoolOptions::new().max_connections(5).connect(&database).await?;
    let min_volume = env_num("MIN_VOLUME_H24_USD", 300_000.0);
    let min_liquidity = env_num("MIN_LIQUIDITY_USD", 0.0);
    let pages = env_num("MAX_PAGES", 3_usize).clamp(1, 10);
    let gap = env_num("REQUEST_SPACING_SECONDS", 7_u64).max(6);
    let every = env_num("SCAN_INTERVAL_SECONDS", 60_u64).max(60);
    let client = reqwest::Client::builder().timeout(Duration::from_secs(25)).user_agent("solana-pump-monitor/0.1").build()?;
    let mut timer = interval(Duration::from_secs(every));
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        timer.tick().await;
        let mut saved = 0usize;
        for p in 1..=pages {
            if p > 1 { sleep(Duration::from_secs(gap)).await; }
            match fetch_page(&client, p).await {
                Ok(json) => {
                    let mut tokens = HashMap::new();
                    if let Some(items) = json["included"].as_array() {
                        for t in items { if let Some(id) = t["id"].as_str() { tokens.insert(id.to_owned(), (t["attributes"]["symbol"].as_str().unwrap_or("").to_owned(), t["attributes"]["name"].as_str().unwrap_or("").to_owned())); } }
                    }
                    if let Some(pools) = json["data"].as_array() {
                        for pool in pools {
                            match save_pool(&db, pool, &tokens, min_volume, min_liquidity).await {
                                Ok(true) => saved += 1,
                                Ok(false) => {},
                                Err(e) => eprintln!("Falha ao salvar pool: {e:#}"),
                            }
                        }
                    }
                },
                Err(e) => { eprintln!("Falha API na página {p}: {e:#}"); if e.to_string().contains("429") { sleep(Duration::from_secs(65)).await; break; } }
            }
        }
        println!("{} UTC | snapshots processados: {}", Utc::now().format("%Y-%m-%d %H:%M:%S"), saved);
    }
}

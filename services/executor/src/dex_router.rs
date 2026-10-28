use anyhow::{bail, Result};
use sqlx::{MySqlPool, Row};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DexType {
    Orca,
}

pub fn get_adapter(dex_id: &str) -> Result<DexType> {
    match dex_id {
        "orca" => Ok(DexType::Orca),
        _ => bail!("DEX adapter not implemented: {}", dex_id),
    }
}

#[derive(Debug, Clone)]
pub struct PoolInfo {
    pub pool_address: String,
    pub dex_id: String,
    pub token_address: String,
    pub quote_token_address: String,
}

pub async fn load_pool(
    db: &MySqlPool,
    pool_address: &str,
) -> Result<PoolInfo> {
    let row = sqlx::query(
        r#"
        SELECT
            pool_address,
            dex_id,
            token_address,
            quote_token_address
        FROM solana_pools
        WHERE pool_address = ?
        LIMIT 1
        "#
    )
    .bind(pool_address)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| {
        anyhow::anyhow!(
            "Pool not found: {}",
            pool_address
        )
    })?;

    let dex_id: String = row.try_get("dex_id")?;
    
    Ok(PoolInfo {
        pool_address: row.try_get("pool_address")?,
        dex_id,
        token_address: row.try_get("token_address")?,
        quote_token_address: row.try_get("quote_token_address")?,
    })
}

pub async fn resolve(
    db: &MySqlPool, 
    a: &str, 
    b: &str, 
    pa: f64, 
    pb: f64) 
-> Result<(PoolInfo, PoolInfo)> {

    if !pa.is_finite() || !pb.is_finite() || pa <= 0.0 || pb <= 0.0 || pa == pb || a == b {
        bail!("Prices or pools invalid");
    }
    let a = load_pool(db, a).await?;
    let b = load_pool(db, b).await?;
    if a.token_address != b.token_address || a.quote_token_address != b.quote_token_address {
        bail!("Pools do not trade the same pair.");
    }
    Ok(if pa < pb { (a,b) } else { (b,a) })
}


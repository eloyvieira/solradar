use anyhow::{bail, Result};
use sqlx::{MySqlPool, Row};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DexType {
    Orca,
    RaydiumClmm,
}

pub fn get_adapter(dex_id: &str) -> Result<DexType> {
    match dex_id {
        "orca" => Ok(DexType::Orca),
        "raydium-clmm" => Ok(DexType::RaydiumClmm),
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
    // Reject unsupported protocols before any RPC swap construction.
    get_adapter(&dex_id)?;
    
    Ok(PoolInfo {
        pool_address: row.try_get("pool_address")?,
        dex_id,
        token_address: row.try_get("token_address")?,
        quote_token_address: row.try_get("quote_token_address")?,
    })
}

pub async fn resolve(
    db: &MySqlPool,
    address_a: &str,
    address_b: &str,
    price_a: f64,
    price_b: f64,
) -> Result<(PoolInfo, PoolInfo)> {

    if address_a == address_b {
        bail!("Buy and sell pools must differ");
    }
    if !price_a.is_finite() || !price_b.is_finite()
        || price_a <= 0.0 || price_b <= 0.0 || price_a == price_b
    {
        bail!("Invalid or equal pool prices");
    }
    let (a, b) = tokio::try_join!(
        load_pool(db, address_a),
        load_pool(db, address_b),
    )?;
    if a.token_address != b.token_address || a.quote_token_address != b.quote_token_address {
        bail!("Pools do not trade the same token/quote pair");
    }
    // Protocol is NOT part of the ordering. Either DEX can be BUY or SELL.
    Ok(if price_a < price_b { (a, b) } else { (b, a) })
}


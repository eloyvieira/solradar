use anyhow::{bail, Result};
use sqlx::{MySqlPool, Row};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DexProtocol {
    PumpSwap,
    MeteoraDlmm,
    OrcaWhirlpool,
    Manifest,
    RaydiumClmm,
    MeteoraDammV2,
    ZeroFi,
    HumidiFi,
    RaydiumAmm,
    JupiterLendAmm,
    PancakeSwapV3,
}

impl DexProtocol {
    pub fn from_dex_id(dex_id: &str) -> Result<Self> {
        match dex_id {
            "pumpswap" => Ok(Self::PumpSwap),
            "meteora" => Ok(Self::MeteoraDlmm),
            "orca" => Ok(Self::OrcaWhirlpool),
            "manifest" => Ok(Self::Manifest),
            "raydium-clmm" => Ok(Self::RaydiumClmm),
            "meteora-damm-v2" => Ok(Self::MeteoraDammV2),
            "zerofi" => Ok(Self::ZeroFi),
            "humidifi" => Ok(Self::HumidiFi),
            "raydium" => Ok(Self::RaydiumAmm),
            "jup-lend-amm" => Ok(Self::JupiterLendAmm),
            "pancakeswap-v3-solana" => Ok(Self::PancakeSwapV3),
            _ => bail!("DEX não suportada: {}", dex_id),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PoolInfo {
    pub pool_address: String,
    pub dex_id: String,
    pub protocol: DexProtocol,
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
        protocol: DexProtocol::from_dex_id(&dex_id)?,
        dex_id,
        token_address: row.try_get("token_address")?,
        quote_token_address: row.try_get("quote_token_address")?,
    })
}

pub async fn resolve_arbitrage(
    db: &MySqlPool,
    pool_a: &str,
    pool_b: &str,
    price_a: f64,
    price_b: f64,
) -> Result<(PoolInfo, PoolInfo)> {
    if !price_a.is_finite()
        || !price_b.is_finite()
        || price_a <= 0.0
        || price_b <= 0.0
        || price_a == price_b
        || pool_a == pool_b
    {
        bail!("Invalid prices or pools");
    }

    let a = load_pool(db, pool_a).await?;
    let b = load_pool(db, pool_b).await?;

    if a.token_address != b.token_address
        || a.quote_token_address != b.quote_token_address
    {
        bail!("Pools do not trade the same pair.");
    }

    if price_a < price_b {
        Ok((a, b))
    } else {
        Ok((b, a))
    }
}

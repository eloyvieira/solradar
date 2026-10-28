use anyhow::{bail, Result};

pub mod orca;
pub mod raydium_clmm;

#[derive(Debug,Clone)] pub struct SwapRequest {
    pub pool_address:String,
    pub mint_in:String,
    pub mint_out:String,
    pub amount_in_atomic:u64,
}

#[derive(Debug,Clone)] pub struct Quote {
    pub amount_out_atomic:u64,
    pub minimum_out_atomic:u64,
    pub estimated_fee_atomic:u64,
    pub slot:u64,
}

pub trait DexAdapter {
    fn protocol(&self) -> &'static str;
    fn quote(&self, _request: &SwapRequest) -> Result<Quote> {
        bail!("DEX adapter not implemented")
    }
}

pub struct UnsupportedDex;
impl DexAdapter for UnsupportedDex {
    fn protocol(&self) -> &'static str { "unsupported" }
}

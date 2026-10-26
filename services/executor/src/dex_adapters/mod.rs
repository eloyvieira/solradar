pub mod orca;
use anyhow::{bail, Result};
use crate::dex_router::PoolInfo;
use orca_whirlpools::SwapInstructions;
use solana_client::nonblocking::rpc_client::RpcClient;

pub async fn build_swap(
    rpc: &RpcClient,
    pool: &PoolInfo,
    input_mint: &str,
    amount_in: u64,
    wallet: &str,
    slippage_bps: u16,
) -> Result<SwapInstructions> {
    match pool.dex_id.as_str() {
        "orca" => {
            orca::build_orca_swap(
                rpc,
                &pool.pool_address,
                input_mint,
                amount_in,
                wallet,
                slippage_bps,
            )
            .await
        }
        dex => bail!(
            "swap adapter not yet implemented: {}",
            dex
        ),
    }
}

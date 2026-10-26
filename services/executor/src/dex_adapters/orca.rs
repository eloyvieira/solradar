use anyhow::{Context, Result};
use orca_whirlpools::{
    swap_instructions,
    SwapConfig,
    SwapInstructions,
    SwapType,
    WhirlpoolDeployment,
};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_pubkey::Pubkey;
use std::str::FromStr;

// Gets the quote and actual instructions
// for an ExactIn swap on Orca Whirlpool.
//
// amount_in must be in the smallest units
// of the input token, not in USD.
pub async fn build_orca_swap(
    rpc: &RpcClient,
    pool_address: &str,
    input_mint: &str,
    amount_in: u64,
    wallet: &str,
    slippage_bps: u16,
) -> Result<SwapInstructions> {

    if amount_in == 0 { anyhow::bail!("Orca: amount_in cannot be zero"); }
    if slippage_bps > 10_000 { anyhow::bail!("Orca: slippage invalid"); }
    let pool = Pubkey::from_str(pool_address).context("Orca: invalid pool address")?;
    let mint = Pubkey::from_str(input_mint).context("Orca: input mint invalid")?;
    let wallet_pubkey = Pubkey::from_str(wallet).context("Orca: invalid wallet")?;

    let config = SwapConfig {
        slippage_tolerance_bps: Some(slippage_bps),
        signer: Some(wallet_pubkey),
        whirlpool_deployment: Some(
            WhirlpoolDeployment::mainnet(),
        ),
    };

    let swap = swap_instructions(
        rpc,
        pool,
        amount_in,
        mint,
        SwapType::ExactIn,
        config,
    )
    .await
    .map_err(|e| {
        anyhow::anyhow!("Orca swap: {}", e)
    })?;

    if swap.instructions.is_empty() {
        anyhow::bail!(
            "Orca: no instructions returned"
        );
    }
    Ok(swap)
}

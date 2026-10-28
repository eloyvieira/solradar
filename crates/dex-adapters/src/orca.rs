use anyhow::{bail, Context, Result};
use orca_whirlpools::{swap_instructions, SwapConfig, SwapQuote, SwapType};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{instruction::Instruction, pubkey::Pubkey};
use std::str::FromStr;

use crate::{Quote, SwapRequest};

pub struct OrcaAdapter;

pub struct OrcaSwap {
    pub quote: Quote,
    pub instructions: Vec<Instruction>,
}

impl OrcaAdapter {
    pub async fn build_exact_in(
        &self,
        rpc: &RpcClient,
        request: &SwapRequest,
        wallet: Pubkey,
        slippage_bps: u16,
    ) -> Result<OrcaSwap> {
        if request.amount_in_atomic == 0 || slippage_bps > 10_000 {
            bail!("Invalid amount or slippage");
        }
        if request.mint_in == request.mint_out {
            bail!("Input and output mint are identical");
        }
        let pool = Pubkey::from_str(&request.pool_address).context("Invalid pool")?;
        let input_mint = Pubkey::from_str(&request.mint_in).context("Invalid input mint")?;

        let result = swap_instructions(
            rpc,
            pool,
            request.amount_in_atomic,
            input_mint,
            SwapType::ExactIn,
            SwapConfig {
                slippage_tolerance_bps: Some(slippage_bps),
                signer: Some(wallet),
                ..Default::default()
            },
        )
        .await
        .map_err(|err| anyhow::anyhow!("Orca swap build failed: {err}"))?;

        if !result.additional_signers.is_empty() {
            bail!("Orca requires additional signers: account handling not yet integrated");
        }

        let quote = match result.quote {
            SwapQuote::ExactIn(q) => Quote {
                amount_out_atomic: q.token_est_out,
                minimum_out_atomic: q.token_min_out,
                estimated_fee_atomic: 0, // TODO: obtain actual fee; zero means unknown here
                slot: rpc.get_slot().await?,
            },
            _ => bail!("Orca SDK returned non-ExactIn quote"),
        };
        if quote.minimum_out_atomic == 0 || result.instructions.is_empty() {
            bail!("Orca returned empty quote or instructions");
        }
        Ok(OrcaSwap { quote, instructions: result.instructions })
    }
}

use anyhow::{bail, Context, Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};
use std::str::FromStr;

const POOL_DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];
const SWAP_V2_DISCRIMINATOR: [u8; 8] = [43, 4, 237, 11, 26, 201, 30, 98];
const RAYDIUM_CLMM_ID: &str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";

#[derive(Debug, Clone)]
pub struct PoolState {
    pub address: Pubkey,
    pub amm_config: Pubkey,
    pub mint_0: Pubkey,
    pub mint_1: Pubkey,
    pub vault_0: Pubkey,
    pub vault_1: Pubkey,
    pub observation: Pubkey,
    pub decimals_0: u8,
    pub decimals_1: u8,
    pub tick_spacing: u16,
    pub liquidity: u128,
    pub sqrt_price_x64: u128,
    pub tick_current: i32,
}

fn pubkey_at(data: &[u8], start: usize) -> Result<Pubkey> {
	let bytes: [u8; 32] = data.get(start..start + 32).context("Truncated pool pubkey")?.try_into()?;
	Ok(Pubkey::new_from_array(bytes))
}

fn u16_at(data: &[u8], start: usize) -> Result<u16> {
	Ok(u16::from_le_bytes(data.get(start..start + 2).context("Truncated pool u16")?.try_into()?))
}

fn u128_at(data: &[u8], start: usize) -> Result<u128> {
    Ok(u128::from_le_bytes(data.get(start..start + 16).context("Truncated pool u128")?.try_into()?))
}

fn i32_at(data: &[u8], start: usize) -> Result<i32> {
    Ok(i32::from_le_bytes(data.get(start..start + 4).context("Truncated pool i32")?.try_into()?))
}

pub fn raydium_program_id() -> Result<Pubkey> {
    Ok(Pubkey::from_str(RAYDIUM_CLMM_ID)?)
}

pub async fn load_pool_state(rpc: &RpcClient, address: Pubkey) -> Result<PoolState> {
    let account = rpc.get_account(&address).await
        .with_context(|| format!("Raydium pool account not found: {address}"))?;

    if account.owner != raydium_program_id()? {
        bail!("Pool {address} is not owned by Raydium CLMM (owner={})", account.owner);
    }

    let d = &account.data;
    if d.len() < 273 || d.get(0..8) != Some(POOL_DISCRIMINATOR.as_slice()) {
        bail!("Invalid or truncated Raydium CLMM PoolState: {address}");
    }

    let state = PoolState {
        address,
        amm_config: pubkey_at(d, 9)?,
        mint_0: pubkey_at(d, 73)?,
        mint_1: pubkey_at(d, 105)?,
        vault_0: pubkey_at(d, 137)?,
        vault_1: pubkey_at(d, 169)?,
        observation: pubkey_at(d, 201)?,
        decimals_0: d[233], decimals_1: d[234],
        tick_spacing: u16_at(d, 235)?,
        liquidity: u128_at(d, 237)?,
        sqrt_price_x64: u128_at(d, 253)?,
        tick_current: i32_at(d, 269)?,
    };

    if state.mint_0 == state.mint_1 || state.tick_spacing == 0 || state.sqrt_price_x64 == 0 {
        bail!("Invalid CLMM pool state");
    }
    Ok(state)
}

#[derive(Debug, Clone)]
pub struct SwapV2Params {
    pub wallet: Pubkey,
    pub mint_in: Pubkey,
    pub mint_out: Pubkey,
    pub user_input_token_account: Pubkey,
    pub user_output_token_account: Pubkey,
    pub token_program: Pubkey,
    pub token_2022_program: Pubkey,
    pub memo_program: Pubkey,
    pub amount_in: u64,
    pub minimum_out: u64,
    pub tick_arrays: Vec<Pubkey>,
    pub bitmap_extension: Option<Pubkey>,
    pub sqrt_price_limit_x64: u128,
}

pub fn build_swap_v2(pool: &PoolState, p: &SwapV2Params) -> Result<Instruction> {
    if p.amount_in == 0 || p.minimum_out == 0 {
        bail!("amount_in and minimum_out must be positive");
    }
    if p.tick_arrays.is_empty() {
        bail!("Raydium CLMM requires ordered initialized tick arrays");
    }
    if p.tick_arrays.len() > 32 {
        bail!("Too many tick arrays");
    }
    let (input_vault, output_vault) = if p.mint_in == pool.mint_0 && p.mint_out == pool.mint_1 {
        (pool.vault_0, pool.vault_1)
    } else if p.mint_in == pool.mint_1 && p.mint_out == pool.mint_0 {
        (pool.vault_1, pool.vault_0)
    } else {
        bail!("Input/output mints do not match Raydium CLMM pool");
    };
    if p.user_input_token_account == p.user_output_token_account {
        bail!("Input and output token accounts are identical");
    }
    let mut accounts = vec![
        AccountMeta::new_readonly(p.wallet, true),
        AccountMeta::new_readonly(pool.amm_config, false),
        AccountMeta::new(pool.address, false),
        AccountMeta::new(p.user_input_token_account, false),
        AccountMeta::new(p.user_output_token_account, false),
        AccountMeta::new(input_vault, false),
        AccountMeta::new(output_vault, false),
        AccountMeta::new(pool.observation, false),
        AccountMeta::new_readonly(p.token_program, false),
        AccountMeta::new_readonly(p.token_2022_program, false),
        AccountMeta::new_readonly(p.memo_program, false),
        AccountMeta::new_readonly(p.mint_in, false),
        AccountMeta::new_readonly(p.mint_out, false),
    ];
    if let Some(bitmap) = p.bitmap_extension {
        accounts.push(AccountMeta::new(bitmap, false));
    }
    for tick_array in &p.tick_arrays {
        accounts.push(AccountMeta::new(*tick_array, false));
    }
    let mut data = Vec::with_capacity(8 + 8 + 8 + 16 + 1);
    data.extend_from_slice(&SWAP_V2_DISCRIMINATOR);
    data.extend_from_slice(&p.amount_in.to_le_bytes());
    data.extend_from_slice(&p.minimum_out.to_le_bytes());
    data.extend_from_slice(&p.sqrt_price_limit_x64.to_le_bytes());
    data.push(1); // is_base_input = true (ExactIn)
    Ok(Instruction { program_id: raydium_program_id()?, accounts, data })
}

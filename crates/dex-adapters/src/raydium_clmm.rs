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

const AMM_CONFIG_SIZE: usize = 117;
const AMM_CONFIG_DISCRIMINATOR: [u8; 8] = [ 218, 244, 33, 104, 203, 203, 43, 111 ];
const FEE_RATE_DENOMINATOR: u32 = 1_000_000;


/*
load_amm_config() — Carregar configurações e taxas da pool.
load_bitmap_extension() — Carregar o bitmap de extensão.
discover_tick_arrays() — Localizar os tick arrays inicializados.
load_tick_arrays() — Carregar e decodificar os tick arrays.
calculate_swap_exact_in() — Calcular a cotação CLMM, incluindo liquidez, ticks e taxas.
calculate_minimum_out() — Aplicar o slippage.
prepare_token_accounts() — Preparar as contas de tokens do usuário.
build_swap_v2() — Corrigir e validar a instrução de swap.
build_exact_in() — Integrar todas as funções e retornar Quote, instruções e signatários.
Executor — Substituir o bail! pela chamada ao adapter.
Validação — Compilar e simular o swap nas duas direções via RPC.
*/

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
pub struct AmmConfig {
    pub address: Pubkey,
    pub bump: u8,
    pub index: u16,
    pub owner: Pubkey,
    pub protocol_fee_rate: u32,
    pub trade_fee_rate: u32,
    pub tick_spacing: u16,
    pub fund_fee_rate: u32,
    pub fund_owner: Pubkey,
}

fn u32_at(data: &[u8], start: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        data.get(start..start + 4)
            .context("Truncated AmmConfig u32")?
            .try_into()?,
    ))
}

fn decode_amm_config(address: Pubkey, data: &[u8]) -> Result<AmmConfig> {
    if data.len() < AMM_CONFIG_SIZE || data.get(..8) != Some(AMM_CONFIG_DISCRIMINATOR.as_slice()) {
        bail!("Invalid or truncated Raydium CLMM AmmConfig: {address}");
    }

    // Official Anchor layout: discriminator[8], bump[1], index[2],
    // owner[32], protocol_fee_rate[4], trade_fee_rate[4], tick_spacing[2],
    // fund_fee_rate[4], padding_u32[4], fund_owner[32], padding[24].
    let config = AmmConfig {
        address,
        bump: data[8],
        index: u16_at(data, 9)?,
        owner: pubkey_at(data, 11)?,
        protocol_fee_rate: u32_at(data, 43)?,
        trade_fee_rate: u32_at(data, 47)?,
        tick_spacing: u16_at(data, 51)?,
        fund_fee_rate: u32_at(data, 53)?,
        fund_owner: pubkey_at(data, 61)?,
    };

    if config.tick_spacing == 0
        || config.trade_fee_rate >= FEE_RATE_DENOMINATOR
        || config.protocol_fee_rate > FEE_RATE_DENOMINATOR
        || config.fund_fee_rate > FEE_RATE_DENOMINATOR
        || config.protocol_fee_rate as u64 + config.fund_fee_rate as u64
            > FEE_RATE_DENOMINATOR as u64
    {
        bail!("Invalid fee rates or tick spacing in Raydium CLMM AmmConfig {address}");
    }
    Ok(config)
}

/// Reads the AmmConfig account referenced by a validated CLMM pool.
/// Owner, discriminator, fee ranges and tick spacing are checked.
pub async fn load_amm_config(rpc: &RpcClient, pool: &PoolState) -> Result<AmmConfig> {
    let address = pool.amm_config;
    let account = rpc.get_account(&address)
        .await
        .with_context(|| format!("Raydium AmmConfig account not found: {address}"))?;

    if account.owner != raydium_program_id()? {
        bail!("AmmConfig {address} is not owned by Raydium CLMM (owner={})", account.owner);
    }

    let config = decode_amm_config(address, &account.data)?;
    if config.tick_spacing != pool.tick_spacing {
        bail!(
            "Raydium CLMM tick spacing mismatch: pool={} config={} ({address})",
            pool.tick_spacing,
            config.tick_spacing
        );
    }
    Ok(config)
}

#[cfg(test)]
mod amm_config_tests {
    use super::*;

    #[test]
    fn decodes_config_offsets_and_fees() {
        let mut bytes = vec![0u8; AMM_CONFIG_SIZE];
        bytes[..8].copy_from_slice(&AMM_CONFIG_DISCRIMINATOR);
        bytes[8] = 254;
        bytes[9..11].copy_from_slice(&7u16.to_le_bytes());
        bytes[43..47].copy_from_slice(&120_000u32.to_le_bytes());
        bytes[47..51].copy_from_slice(&2_500u32.to_le_bytes());
        bytes[51..53].copy_from_slice(&60u16.to_le_bytes());
        bytes[53..57].copy_from_slice(&40_000u32.to_le_bytes());
        let result = decode_amm_config(Pubkey::new_unique(), &bytes).unwrap();
        assert_eq!(result.index, 7);
        assert_eq!(result.bump, 254);
        assert_eq!(result.protocol_fee_rate, 120_000);
        assert_eq!(result.trade_fee_rate, 2_500);
        assert_eq!(result.fund_fee_rate, 40_000);
        assert_eq!(result.tick_spacing, 60);
    }

    #[test]
    fn rejects_invalid_config() {
        let mut bytes = vec![0u8; AMM_CONFIG_SIZE];
        bytes[..8].copy_from_slice(&AMM_CONFIG_DISCRIMINATOR);
        bytes[47..51].copy_from_slice(&1_000_000u32.to_le_bytes());
        bytes[51..53].copy_from_slice(&60u16.to_le_bytes());
        assert!(decode_amm_config(Pubkey::new_unique(), &bytes).is_err());
        bytes[47..51].copy_from_slice(&2_500u32.to_le_bytes());
        assert!(decode_amm_config(Pubkey::new_unique(), &bytes).is_ok());
        bytes[0] = 0;
        assert!(decode_amm_config(Pubkey::new_unique(), &bytes).is_err());
    }
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

pub const TICKS_PER_ARRAY: i32 = 60;

pub fn tick_array_address(pool: Pubkey, start_tick: i32) -> Result<Pubkey> {
    let program = raydium_program_id()?;
    Ok(Pubkey::find_program_address(
        &[b"tick_array", pool.as_ref(), &start_tick.to_be_bytes()],
        &program,
    ).0)
}

pub fn tick_array_start(tick: i32, tick_spacing: u16) -> Result<i32> {
    if tick_spacing == 0 { bail!("tick_spacing must be positive"); }
    let width = i32::from(tick_spacing) * TICKS_PER_ARRAY;
    Ok(tick.div_euclid(width) * width)
}

pub async fn load_tick_array(
    rpc: &RpcClient,
    pool: Pubkey,
    start_tick: i32,
) -> Result<(Pubkey, Vec<u8>)> {
    let address = tick_array_address(pool, start_tick)?;
    let account = rpc.get_account(&address).await
        .with_context(|| format!("Raydium tick array unavailable: {address}"))?;
    if account.owner != raydium_program_id()? {
        bail!("Tick array {address} has incorrect owner");
    }
    let d = &account.data;
    if d.len() < 44 {
        bail!("Truncated tick array: {address}");
    }
    let owner_pool = pubkey_at(d, 8)?;
    if owner_pool != pool {
        bail!("Tick array {address} belongs to a different pool");
    }
    if i32_at(d, 40)? != start_tick {
        bail!("Tick array {address} has unexpected start index");
    }
    Ok((address, d.clone()))
}



const POOL_BITMAP_OFFSET: usize = 904;
const POOL_BITMAP_BITS: usize = 1024;
const EXT_BITMAP_SEED: &[u8] = b"pool_tick_array_bitmap_extension";
const EXT_BITMAP_GROUPS: usize = 14;
const EXT_BITS_PER_GROUP: usize = 512;
fn bitmap_indices(data: &[u8], offset: usize, bits: usize) -> Result<Vec<usize>> {
    let byte_len = bits / 8;
    let bitmap = data
        .get(offset..offset + byte_len)
        .context("Raydium bitmap account is truncated")?;

    let mut indices = Vec::new();
    for (byte_index, byte) in bitmap.iter().enumerate() {
        if *byte == 0 {
            continue;
        }
        for bit in 0..8 {
            if byte & (1u8 << bit) != 0 {
                indices.push(byte_index * 8 + bit);
            }
        }
    }
    Ok(indices)
}

pub fn tick_array_bitmap_extension_address(pool: Pubkey) -> Result<Pubkey> {
    let program_id = raydium_program_id()?;
    Ok(Pubkey::find_program_address(
        &[EXT_BITMAP_SEED, pool.as_ref()],
        &program_id,
    ).0)
}

pub async fn discover_tick_arrays(
    rpc: &RpcClient,
    pool: &PoolState,
    zero_for_one: bool,
    max_arrays: usize,
) -> Result<(Vec<Pubkey>, Option<Pubkey>)> {
    if max_arrays == 0 || max_arrays > 32 {
        bail!("max_arrays must be between 1 and 32");
    }
    if pool.tick_spacing == 0 {
        bail!("Invalid Raydium tick spacing");
    }
    let program_id = raydium_program_id()?;
    let account = rpc.get_account(&pool.address).await
        .with_context(|| format!("Cannot load Raydium pool {}", pool.address))?;

    if account.owner != program_id {
        bail!("Invalid Raydium pool owner");
    }

    if account.data.get(..8) != Some(POOL_DISCRIMINATOR.as_slice()) {
        bail!("Invalid Raydium pool discriminator");
    }

    let spacing = i32::from(pool.tick_spacing);
    let width = spacing
        .checked_mul(TICKS_PER_ARRAY)
        .context("Tick array width overflow")?;

    let current_start = tick_array_start(
        pool.tick_current,
        pool.tick_spacing,
    )?;

    let mut starts: Vec<i32> = Vec::new();

    for bit in bitmap_indices(
        &account.data,
        POOL_BITMAP_OFFSET,
        POOL_BITMAP_BITS,
    )? {
        let array_index = bit as i32 - 512;
        starts.push(array_index * width);
    }

    let extension_address = tick_array_bitmap_extension_address(pool.address)?;
    let mut bitmap_extension = None;

    let extension_account = rpc.get_account(&extension_address).await;

    match extension_account {
        Ok(extension) => {
            if extension.owner != program_id {
                bail!("Invalid Raydium bitmap extension owner");
            }

            const GROUP_BYTES: usize = 64;
            const GROUP_COUNT: usize = EXT_BITMAP_GROUPS;
            const POSITIVE_OFFSET: usize = 8 + 32;
            const NEGATIVE_OFFSET: usize =
                POSITIVE_OFFSET + GROUP_COUNT * GROUP_BYTES;

            if extension.data.len() < NEGATIVE_OFFSET + GROUP_COUNT * GROUP_BYTES {
                bail!("Truncated Raydium bitmap extension");
            }

            let extension_pool = pubkey_at(&extension.data, 8)?;
            if extension_pool != pool.address {
                bail!("Bitmap extension belongs to another pool");
            }

            for bit in bitmap_indices(
                &extension.data,
                POSITIVE_OFFSET,
                GROUP_COUNT * EXT_BITS_PER_GROUP,
            )? {
                let array_index = 512i32 + bit as i32;
                let start = array_index * width;

                if start <= 443636 {
                    starts.push(start);
                }
            }

            for bit in bitmap_indices(
                &extension.data,
                NEGATIVE_OFFSET,
                GROUP_COUNT * EXT_BITS_PER_GROUP,
            )? {
                let array_index = -513i32 - bit as i32;
                let start = array_index * width;

                if start >= -443636 {
                    starts.push(start);
                }
            }

            bitmap_extension = Some(extension_address);
        }

        Err(solana_client::client_error::ClientError {
            kind: solana_client::client_error::ClientErrorKind::RpcError(
                solana_client::rpc_request::RpcError::ForUser(message),
            ),
            ..
        }) if message.contains("AccountNotFound") => {}

        Err(error) => {
            return Err(error).context("Failed loading bitmap extension");
        }
    }

    if zero_for_one {
        starts.retain(|start| *start <= current_start);
        starts.sort_unstable_by(|a, b| b.cmp(a));
    } else {
        starts.retain(|start| *start >= current_start);
        starts.sort_unstable();
    }

    starts.dedup();
    starts.truncate(max_arrays);

    if starts.is_empty() {
        bail!(
            "No initialized Raydium tick arrays found for pool {}",
            pool.address
        );
    }

    let mut addresses = Vec::with_capacity(starts.len());

    for start in starts {
        addresses.push(tick_array_address(pool.address, start)?);
    }

    Ok((addresses, bitmap_extension))
}



const TICK_ARRAY_DISCRIMINATOR: [u8; 8] = [192, 155, 85, 205, 49, 249, 129, 42];
const TICK_STATE_SIZE: usize = 168;
const TICK_ARRAY_HEADER_SIZE: usize = 44;
const TICK_ARRAY_ACCOUNT_SIZE: usize = 10240;

#[derive(Debug, Clone)]
pub struct DecodedTick {
    pub tick: i32,
    pub liquidity_net: i128,
    pub liquidity_gross: u128,
    pub fee_growth_outside_0_x64: u128,
    pub fee_growth_outside_1_x64: u128,
    pub reward_growths_outside_x64: [u128; 3],
    pub order_phase: u64,
    pub orders_amount: u64,
    pub part_filled_orders_remaining: u64,
    pub unfilled_ratio_x64: u128,
}

#[derive(Debug, Clone)]
pub struct DecodedTickArray {
    pub pool: Pubkey,
    pub start_tick_index: i32,
    pub initialized_tick_count: u8,
    pub ticks: Vec<DecodedTick>,
}

fn i128_at(data: &[u8], offset: usize) -> Result<i128> {
    let bytes: [u8; 16] = data
        .get(offset..offset + 16)
        .context("Truncated i128")?
        .try_into()?;

    Ok(i128::from_le_bytes(bytes))
}

fn u64_at(data: &[u8], offset: usize) -> Result<u64> {
    let bytes: [u8; 8] = data
        .get(offset..offset + 8)
        .context("Truncated u64")?
        .try_into()?;

    Ok(u64::from_le_bytes(bytes))
}

pub fn decode_tick_array(
    data: &[u8],
    pool: Pubkey,
    tick_spacing: u16,
) -> Result<DecodedTickArray> {
    if tick_spacing == 0 {
        bail!("Invalid tick spacing");
    }

    if data.len() < TICK_ARRAY_ACCOUNT_SIZE {
        bail!("Truncated Raydium TickArrayState");
    }

    if data.get(..8) != Some(TICK_ARRAY_DISCRIMINATOR.as_slice()) {
        bail!("Invalid Raydium TickArrayState discriminator");
    }

    let owner_pool = pubkey_at(data, 8)?;

    if owner_pool != pool {
        bail!("Tick array belongs to another pool");
    }

    let start_tick_index = i32_at(data, 40)?;
    let width = i32::from(tick_spacing) * TICKS_PER_ARRAY;

    if start_tick_index % width != 0 {
        bail!("Invalid tick array start index");
    }

    let count_offset =
        TICK_ARRAY_HEADER_SIZE + TICK_STATE_SIZE * 60;

    let initialized_tick_count = data[count_offset];

    if initialized_tick_count > 60 {
        bail!("Invalid initialized tick count");
    }

    let mut ticks = Vec::new();

    for index in 0..60usize {
        let offset =
            TICK_ARRAY_HEADER_SIZE + index * TICK_STATE_SIZE;

        let tick = i32_at(data, offset)?;
        let liquidity_net = i128_at(data, offset + 4)?;
        let liquidity_gross = u128_at(data, offset + 20)?;

        // Current Raydium layout includes limit-order fields.
        let order_phase = u64_at(data, offset + 116)?;
        let orders_amount = u64_at(data, offset + 124)?;
        let part_filled_orders_remaining =
            u64_at(data, offset + 132)?;

        let initialized = liquidity_gross > 0
            || orders_amount > 0
            || part_filled_orders_remaining > 0;

        if !initialized {
            continue;
        }

        let expected_tick = start_tick_index
            + (index as i32) * i32::from(tick_spacing);

        if tick != expected_tick {
            bail!(
                "Invalid tick index: expected {}, got {}",
                expected_tick,
                tick
            );
        }

        ticks.push(DecodedTick {
            tick,
            liquidity_net,
            liquidity_gross,
            fee_growth_outside_0_x64:
                u128_at(data, offset + 36)?,
            fee_growth_outside_1_x64:
                u128_at(data, offset + 52)?,
            reward_growths_outside_x64: [
                u128_at(data, offset + 68)?,
                u128_at(data, offset + 84)?,
                u128_at(data, offset + 100)?,
            ],
            order_phase,
            orders_amount,
            part_filled_orders_remaining,
            unfilled_ratio_x64:
                u128_at(data, offset + 140)?,
        });
    }

    if ticks.len() != initialized_tick_count as usize {
        bail!(
            "Initialized tick count mismatch: header={}, decoded={}",
            initialized_tick_count,
            ticks.len()
        );
    }

    Ok(DecodedTickArray {
        pool,
        start_tick_index,
        initialized_tick_count,
        ticks,
    })
}



use solana_clmm_raydium::{
    compute_swap_full,
    InitializedTick,
    SwapPool,
};

#[derive(Debug, Clone)]
pub struct ExactInQuote {
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee_amount: u64,
    pub final_sqrt_price_x64: u128,
    pub final_tick: i32,
}

pub fn calculate_swap_exact_in(
    pool: &PoolState,
    config: &AmmConfig,
    tick_arrays: &[DecodedTickArray],
    amount_in: u64,
    zero_for_one: bool,
) -> Result<ExactInQuote> {
    if amount_in == 0 {
        bail!("amount_in must be positive");
    }

    if tick_arrays.is_empty() {
        bail!("No Raydium tick arrays supplied");
    }

    if config.tick_spacing != pool.tick_spacing {
        bail!("Tick spacing mismatch");
    }

    let mut initialized_ticks = Vec::new();
    let mut limit_order_ticks: Vec<i32> = Vec::new();
    for array in tick_arrays {
        if array.pool != pool.address {
            bail!("Tick array belongs to another pool");
        }

        for tick in &array.ticks {
            if tick.orders_amount > 0
                || tick.part_filled_orders_remaining > 0
            {
                limit_order_ticks.push(tick.tick);
            }
            if tick.liquidity_gross > 0 {
                initialized_ticks.push(InitializedTick {
                    tick: tick.tick,
                    liquidity_net: tick.liquidity_net,
                });
            }
        }
    }

    initialized_ticks.sort_unstable_by_key(|t| t.tick);
    for pair in initialized_ticks.windows(2) {
        if pair[0].tick == pair[1].tick {
            bail!("Duplicate initialized Raydium tick");
        }
    }

    let swap_pool = SwapPool {
        sqrt_price_x64: pool.sqrt_price_x64,
        liquidity: pool.liquidity,
        tick_current: pool.tick_current,
        tick_spacing: pool.tick_spacing,
        fee_rate_pips: config.trade_fee_rate,
    };

    let result = compute_swap_full(
        &swap_pool,
        &initialized_ticks,
        amount_in,
        0,
        true, // ExactIn
        zero_for_one,
    )
    .map_err(|e| anyhow::anyhow!(
        "Raydium swap math failed: {e:?}"
    ))?;

    // Verifica ordens limite somente no caminho estimado do swap.
    // Se existir uma ordem no caminho, a cotação padrão não é confiável.
    let start_tick = pool.tick_current;
    let end_tick = result.final_tick;
    let min_tick = start_tick.min(end_tick);
    let max_tick = start_tick.max(end_tick);
    for tick in &limit_order_ticks {
        if *tick >= min_tick && *tick <= max_tick {
            bail!(
                "Raydium swap crosses limit order at tick {} \
                 (current={}, final={}). \
                 Full limit-order math required",
                tick,
                start_tick,
                end_tick
            );
        }
    }

    if result.amount_in != amount_in {
        bail!(
            "Incomplete Raydium quote: consumed {} of {}",
            result.amount_in,
            amount_in
        );
    }

    if result.amount_out == 0 {
        bail!("Raydium quote returned zero output");
    }

    Ok(ExactInQuote {
        amount_in: result.amount_in,
        amount_out: result.amount_out,
        fee_amount: result.fee_amount,
        final_sqrt_price_x64: result.final_sqrt_price_x64,
        final_tick: result.final_tick,
    })
}



pub fn calculate_minimum_out(
    expected_out: u64,
    slippage_bps: u16,
) -> Result<u64> {
    if expected_out == 0 {
        bail!("Expected output must be positive");
    }

    if slippage_bps >= 10_000 {
        bail!("Invalid slippage: {} bps", slippage_bps);
    }

    let minimum = (expected_out as u128)
        .checked_mul(10_000u128 - slippage_bps as u128)
        .context("Slippage multiplication overflow")?
        / 10_000u128;

    if minimum == 0 {
        bail!("Minimum output rounded to zero");
    }

    Ok(u64::try_from(minimum)?)
}



#[derive(Debug, Clone)]
pub struct BuiltRaydiumSwap {
    pub quote: ExactInQuote,
    pub minimum_out: u64,
    pub instructions: Vec<Instruction>,
    pub tick_arrays: Vec<Pubkey>,
}

pub async fn build_exact_in(
    rpc: &RpcClient,
    pool_address: Pubkey,
    wallet: Pubkey,
    mint_in: Pubkey,
    mint_out: Pubkey,
    user_input_token_account: Pubkey,
    user_output_token_account: Pubkey,
    token_program: Pubkey,
    token_2022_program: Pubkey,
    memo_program: Pubkey,
    amount_in: u64,
    slippage_bps: u16,
) -> Result<BuiltRaydiumSwap> {
    if amount_in == 0 {
        bail!("Amount must be positive");
    }

    // 1. Carrega pool e configuração.
    let pool = load_pool_state(rpc, pool_address).await?;
    let config = load_amm_config(rpc, &pool).await?;

    // 2. Identifica a direção.
    let zero_for_one =
        if mint_in == pool.mint_0 && mint_out == pool.mint_1 {
            true
        } else if mint_in == pool.mint_1
            && mint_out == pool.mint_0
        {
            false
        } else {
            bail!("Mints do not match Raydium pool");
        };

    // 3. Descobre os tick arrays.
    let (tick_addresses, bitmap_extension) =
        discover_tick_arrays(
            rpc,
            &pool,
            zero_for_one,
            8,
        )
        .await?;

    // 4. Carrega e decodifica os arrays.
    let mut decoded_arrays = Vec::new();

    for address in &tick_addresses {
        let account = rpc.get_account(address).await
            .with_context(|| {
                format!("Cannot load tick array {address}")
            })?;

        if account.owner != raydium_program_id()? {
            bail!("Invalid tick array owner: {address}");
        }

        let decoded = decode_tick_array(
            &account.data,
            pool.address,
            pool.tick_spacing,
        )?;

        if tick_array_address(
            pool.address,
            decoded.start_tick_index,
        )? != *address {
            bail!("Tick array address mismatch: {address}");
        }

        decoded_arrays.push(decoded);
    }

    // 5. Calcula o swap.
    let quote = calculate_swap_exact_in(
        &pool,
        &config,
        &decoded_arrays,
        amount_in,
        zero_for_one,
    )?;

    // 6. Garante que o preço final permaneceu
    // dentro de um array efetivamente carregado.
    let spacing = i32::from(pool.tick_spacing);
    let width = spacing
        .checked_mul(TICKS_PER_ARRAY)
        .context("Tick array width overflow")?;

    let final_tick_covered = decoded_arrays.iter().any(|a| {
        a.start_tick_index <= quote.final_tick
            && quote.final_tick
                < a.start_tick_index + width
    });

    if !final_tick_covered {
        bail!(
            "Quote crosses beyond loaded tick arrays; \
             more arrays are required"
        );
    }

    // 7. Aplica o slippage.
    let minimum_out = calculate_minimum_out(
        quote.amount_out,
        slippage_bps,
    )?;

    let params = SwapV2Params {
        wallet,
        mint_in,
        mint_out,
        user_input_token_account,
        user_output_token_account,
        token_program,
        token_2022_program,
        memo_program,
        amount_in,
        minimum_out,
        tick_arrays: tick_addresses.clone(),
        bitmap_extension,
        sqrt_price_limit_x64: 0,
    };

    let instruction = build_swap_v2(&pool, &params)?;

    Ok(BuiltRaydiumSwap {
        quote,
        minimum_out,
        instructions: vec![instruction],
        tick_arrays: tick_addresses,
    })
}


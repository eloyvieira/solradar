use anyhow::{bail, Context, Result};
use chrono::Utc;
use futures_util::StreamExt;
use lapin::options::{BasicAckOptions, BasicConsumeOptions, BasicQosOptions};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{signature::{read_keypair_file, Keypair}, signer::Signer};
use solradar_common::{channel, config, Signal};
use solradar_dex_adapters::{orca::OrcaAdapter, Quote, SwapRequest};
use solradar_dex_adapters::raydium_clmm;
use solana_sdk::{instruction::Instruction, pubkey::Pubkey, system_instruction};
use std::collections::HashSet;
use std::str::FromStr;
use sqlx::{MySqlPool, Row};
use std::{env, sync::Arc};
use uuid::Uuid;
use crate::{dex_router, transaction_builder};
use solana_address_lookup_table_interface::state::AddressLookupTable;
use solana_message::AddressLookupTableAccount;

use spl_token_2022::ID as TOKEN_2022_PROGRAM_ID;
use spl_associated_token_account::ID as ASSOCIATED_TOKEN_PROGRAM_ID;
use spl_token::ID as TOKEN_PROGRAM_ID;
use spl_memo::ID as MEMO_PROGRAM_ID;

use orca_whirlpools::{
    set_native_mint_wrapping_strategy,
    set_enforce_token_balance_check,
    NativeMintWrappingStrategy,
};

struct Runtime {
    rpc: RpcClient,
    wallet: solana_sdk::signature::Keypair,
    allowed_quote: String,
    amount_raw: u64,
    token_amount_raw: u64,
    direction: Direction,
    slippage_bps: u16,
    compute_limit: u32,
    compute_price: u64,
    simulate_ignore_profit: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction { BuySell, SellBuy, Auto }

impl Direction {
    fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_uppercase().as_str() {
            "BUY_SELL" => Ok(Self::BuySell),
            "SELL_BUY" => Ok(Self::SellBuy),
            "AUTO" => Ok(Self::Auto),
            _ => bail!("Invalid ARBITRAGE_DIRECTION: {s}. Use BUY_SELL, SELL_BUY or AUTO"),
        }
    }
    fn name(self) -> &'static str {
        match self { Self::BuySell => "BUY_SELL", Self::SellBuy => "SELL_BUY", Self::Auto => "AUTO" }
    }
}

async fn load_lookup_tables(
    rpc: &RpcClient,
) -> Result<Vec<AddressLookupTableAccount>> {
    let addresses = env::var("ARBITRAGE_LOOKUP_TABLES").unwrap_or_default();
    let mut tables = Vec::new();
    for address in addresses
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let key = Pubkey::from_str(address).with_context(|| format!("Invalid ALT address: {address}"))?;
        let account = rpc.get_account(&key).await.with_context(|| format!("Failed loading ALT {key}"))?;
        let table = AddressLookupTable::deserialize(&account.data).with_context(|| format!("Invalid ALT data: {key}"))?;
        tables.push(AddressLookupTableAccount {
            key,
            addresses: table.addresses.to_vec(),
        });
    }

    Ok(tables)
}

async fn log_step(db: &MySqlPool, signal_id: &str, order_id: Option<&str>, step: &str, status: &str, details: Option<&str>) -> Result<()> {
    sqlx::query("INSERT INTO trade_order_logs (signal_id,order_id,step,status,details,created_at) VALUES (?,?,?,?,?,UTC_TIMESTAMP(6))")
        .bind(signal_id)
        .bind(order_id)
        .bind(step)
        .bind(status)
        .bind(details)
        .execute(db)
        .await?;
    Ok(())
}

async fn update(db: &MySqlPool, order: &str, signal: &str, status: &str, signature: Option<&str>, error: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE trade_orders SET status=?,tx_signature=?,error_message=? WHERE id=?")
        .bind(status)
        .bind(signature)
        .bind(error)
        .bind(order)
        .execute(db)
        .await?;

    sqlx::query("UPDATE signals_arbitrage SET status=? WHERE id=?")
        .bind(status)
        .bind(signal)
        .execute(db)
        .await?;
    Ok(())
}


async fn raydium_token_account(
    rpc: &RpcClient,
    wallet: Pubkey,
    mint: Pubkey,
) -> Result<(Pubkey, Pubkey)> {
    let token_program = TOKEN_PROGRAM_ID;
    let token_2022_program = TOKEN_2022_PROGRAM_ID;
    let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID;

    let mint_account = rpc
        .get_account(&mint)
        .await
        .with_context(|| format!("Raydium: failed loading mint {mint}"))?;

    let program = mint_account.owner;

    if program != token_program && program != token_2022_program {
        bail!("Unsupported token program for mint {mint}: {program}");
    }

    let ata = Pubkey::find_program_address(
        &[
            wallet.as_ref(),
            program.as_ref(),
            mint.as_ref(),
        ],
        &associated_token_program,
    ).0;

    Ok((ata, program))
}

// Instruções de preparação compartilhadas pelos dois swaps.
// Criam ATAs ausentes sem enviar transação separada.
async fn prepare_token_accounts(
    cfg: &Runtime,
    first_mint: Pubkey,
    initial_amount: u64,
    first_dex_is_raydium: bool,
    raydium_mints: &[Pubkey],
) -> Result<Vec<Instruction>> {
    let wallet = cfg.wallet.pubkey();
    let mut instructions = Vec::new();
    let mut seen = HashSet::new();

    for &mint in raydium_mints {
        if !seen.insert(mint) { continue; }
        let (ata, program) = raydium_token_account(&cfg.rpc, wallet, mint).await
            .with_context(|| format!("Preparing ATA for mint {mint}"))?;

        instructions.push(
            spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                &wallet, &wallet, &mint, &program,
            ),
        );

        // Somente a PRIMEIRA perna Raydium com entrada wSOL precisa
        // de saldo inicial em sua ATA. A segunda recebe tokens da primeira.
        if first_dex_is_raydium && mint == first_mint
            && mint == spl_token::native_mint::ID
        {
            // Se a ATA ainda não existir, consideramos saldo zero.
            // Qualquer outro erro de RPC não deve ser ignorado.
            let existing_balance = match cfg.rpc.get_account(&ata).await {
                Ok(account) => {
                    if account.owner != program {
                        bail!("wSOL ATA {ata} has unexpected owner");
                    }
                    if account.data.len() < 72 {
                        bail!("Invalid wSOL ATA data length");
                    }
                    u64::from_le_bytes(account.data[64..72].try_into()?)
                }
                Err(err) => {
                    let message = err.to_string();
                    if message.contains("AccountNotFound") { 0 }
                    else { return Err(err.into()); }
                }
            };
            let deficit = initial_amount.saturating_sub(existing_balance);
            if deficit > 0 {
                instructions.push(system_instruction::transfer(&wallet, &ata, deficit));
                instructions.push(spl_token::instruction::sync_native(&program, &ata)?);
            }
        }
    }
    Ok(instructions)
}

// Single representation of a swap regardless of the DEX protocol.
struct BuiltSwap {
    quote: Quote,
    instructions: Vec<Instruction>,
    additional_signers: Vec<Keypair>,
}

async fn build_swap(
    cfg: &Runtime,
    dex_id: &str,
    request: &SwapRequest,
) -> Result<BuiltSwap> {
    match dex_router::get_adapter(dex_id)? {
        dex_router::DexType::Orca => {
            let swap = OrcaAdapter
                .build_exact_in(
                    &cfg.rpc,
                    request,
                    cfg.wallet.pubkey(),
                    cfg.slippage_bps,
                )
                .await?;
            Ok(BuiltSwap {
                quote: swap.quote,
                instructions: swap.instructions,
                additional_signers: swap.additional_signers,
            })
        }

        dex_router::DexType::RaydiumClmm => {
            let pool = Pubkey::from_str(&request.pool_address).context("Invalid Raydium CLMM pool address")?;
            let mint_in = Pubkey::from_str(&request.mint_in).context("Invalid Raydium input mint")?;
            let mint_out = Pubkey::from_str(&request.mint_out).context("Invalid Raydium output mint")?;

            let state = raydium_clmm::load_pool_state(
                &cfg.rpc,
                pool,
            ).await?;
            if !(
                (mint_in == state.mint_0 && mint_out == state.mint_1)
                ||
                (mint_in == state.mint_1 && mint_out == state.mint_0)
            ) {
                bail!(
                    "Raydium CLMM pool {} does not trade {}/{}",
                    pool,
                    mint_in,
                    mint_out
                );
            }

            let (input_ata, input_program) = raydium_token_account(
                &cfg.rpc,
                cfg.wallet.pubkey(),
                mint_in,
            )
            .await
            .context("Raydium: preparing input token account")?;

            let (output_ata, output_program) = raydium_token_account(
                &cfg.rpc,
                cfg.wallet.pubkey(),
                mint_out,
            )
            .await
            .context("Raydium: preparing output token account")?;

            let token_program = TOKEN_PROGRAM_ID;

            let token_2022_program = TOKEN_2022_PROGRAM_ID;

            let memo_program = MEMO_PROGRAM_ID;

            // A criação das ATAs é incluída no início da transação atômica,
            // não feita aqui. A cotação não depende de elas já existirem.
            let _ = (input_program, output_program);

            // Chama a função no adapter.
            let swap = raydium_clmm::build_exact_in(
                &cfg.rpc,
                pool,
                cfg.wallet.pubkey(),
                mint_in,
                mint_out,
                input_ata,
                output_ata,
                token_program,
                token_2022_program,
                memo_program,
                request.amount_in_atomic,
                cfg.slippage_bps,
            )
            .await
            .with_context(|| {
                format!(
                    "Raydium build_exact_in failed: pool={}, input={}, output={}",
                    pool, mint_in, mint_out
                )
            })?;

            let quote = Quote {
                amount_out_atomic: swap.quote.amount_out,
                minimum_out_atomic: swap.minimum_out,
                estimated_fee_atomic: swap.quote.fee_amount,
                slot: cfg.rpc.get_slot().await?,
            };

            if quote.minimum_out_atomic == 0
                || swap.instructions.is_empty()
            {
                bail!("Raydium returned empty quote or instructions");
            }

            Ok(BuiltSwap {
                quote,
                instructions: swap.instructions,
                additional_signers: Vec::new(),
            })
        }
    }
}

async fn process(db: &MySqlPool, cfg: &Runtime, sig: Signal) -> Result<()> {
    if sig.strategy != "ARBITRAGE" { bail!("Unsupported strategy: {}", sig.strategy); }

    let row = sqlx::query("SELECT token_address,pool_address,secondary_pool,CAST(price_a AS CHAR) pa,CAST(price_b AS CHAR) pb,observed_at,status FROM signals_arbitrage WHERE id=?")
        .bind(&sig.id)
        .fetch_optional(db)
        .await?
        .context("Signal not found")?;

    let status: String = row.try_get("status")?;
    if status != "OBSERVED" { return Ok(()); }
    let token: String = row.try_get("token_address")?;
    let a: String = row.try_get("pool_address")?;
    let b: String = row.try_get("secondary_pool")?;
    let pa: f64 = row.try_get::<String,_>("pa")?.parse()?;
    let pb: f64 = row.try_get::<String,_>("pb")?.parse()?;
    let observed_at: chrono::NaiveDateTime = row.try_get("observed_at")?;
    let age = Utc::now().naive_utc().signed_duration_since(observed_at).num_seconds();
    let max_age: i64 = config("MAX_SIGNAL_AGE_SECONDS", 30_i64);
    if age < 0 || age > max_age { bail!("Signal stale: {} seconds", age); }

    let (buy_pool, sell_pool) = dex_router::resolve(db, &a, &b, pa, pb).await?;
    dex_router::get_adapter(&buy_pool.dex_id)?;
    dex_router::get_adapter(&sell_pool.dex_id)?;
    if  buy_pool.token_address != token 
        || sell_pool.token_address != token
        || buy_pool.quote_token_address != cfg.allowed_quote 
        || sell_pool.quote_token_address != cfg.allowed_quote {
        bail!("Unexpected token/quote pair");
    }

    let order_id = Uuid::new_v4().to_string();
    let insert = sqlx::query("INSERT IGNORE INTO trade_orders(id,signal_id,strategy,token_address,pool_address,status,mode) VALUES(?,?,?,?,?,?,?)")
        .bind(&order_id)
        .bind(&sig.id)
        .bind("ARBITRAGE")
        .bind(&token)
        .bind(&buy_pool.pool_address)
        .bind("PROCESSING")
        .bind("PAPER")
        .execute(db)
        .await?;
    if insert.rows_affected() == 0 { return Ok(()); }

    log_step(db, &sig.id, Some(&order_id), "ORDER_CREATED", "OK",
        Some(&serde_json::json!({
            "mode": "PAPER", "sent": false,
            "buy": {"pool": buy_pool.pool_address, "dex": buy_pool.dex_id},
            "sell": {"pool": sell_pool.pool_address, "dex": sell_pool.dex_id}
        }).to_string())).await?;
    let result: Result<()> = async {

        // BUY_SELL: quote -> token -> quote (buy cheap, sell expensive).
        // SELL_BUY: token -> quote -> token (sell expensive, buy cheap).
        // AUTO: evaluate both and select the greater projected percentage return.
        let candidates: &[Direction] = match cfg.direction {
            Direction::Auto => &[Direction::BuySell, Direction::SellBuy],
            Direction::BuySell => &[Direction::BuySell],
            Direction::SellBuy => &[Direction::SellBuy],
        };

        let mut failures: Vec<String> = Vec::new();
        let mut winner: Option<(Direction, BuiltSwap, BuiltSwap, u64, f64)> = None;
        
        for direction in candidates.iter().copied() {
            let (first_pool, second_pool, first_mint, second_mint, initial_amount) = match direction {
                Direction::BuySell => (&buy_pool, &sell_pool, &cfg.allowed_quote, &token, cfg.amount_raw),
                Direction::SellBuy => (&sell_pool, &buy_pool, &token, &cfg.allowed_quote, cfg.token_amount_raw),
                Direction::Auto => unreachable!(),
            };

            if initial_amount == 0 {
                failures.push(format!("{}: initial amount is zero", direction.name()));
                continue;
            }

            let attempt: Result<(BuiltSwap, BuiltSwap)> = async {
                let first_request = SwapRequest {
                    pool_address: first_pool.pool_address.clone(),
                    mint_in: first_mint.clone(),
                    mint_out: second_mint.clone(),
                    amount_in_atomic: initial_amount,
                };

                log_step(db,&sig.id,Some(&order_id),"DEX_FIRST_BUILD","STARTED",Some(&serde_json::json!({
                    "direction":direction.name(),
                    "dex":first_pool.dex_id,
                    "pool":first_pool.pool_address,
                    "mint_in":first_request.mint_in,
                    "mint_out":first_request.mint_out,
                    "amount":initial_amount
                }).to_string())).await?;
                let first = build_swap(cfg, &first_pool.dex_id, &first_request).await?;
                log_step(db,&sig.id,Some(&order_id),"DEX_FIRST_QUOTE","OK",Some(&serde_json::json!({
                    "direction":direction.name(),
                    "min_out":first.quote.minimum_out_atomic,
                    "expected_out":first.quote.amount_out_atomic,
                    "slot":first.quote.slot
                }).to_string())).await?;

                let second_request = SwapRequest {
                    pool_address: second_pool.pool_address.clone(),
                    mint_in: second_mint.clone(),
                    mint_out: first_mint.clone(),
                    amount_in_atomic: first.quote.minimum_out_atomic,
                };
                log_step(db,&sig.id,Some(&order_id),"DEX_SECOND_BUILD","STARTED",Some(&serde_json::json!({
                    "direction":direction.name(),
                    "dex":second_pool.dex_id,
                    "pool":second_pool.pool_address,
                    "mint_in":second_request.mint_in,
                    "mint_out":second_request.mint_out,
                    "amount":second_request.amount_in_atomic
                }).to_string())).await?;

                let second = build_swap(cfg, &second_pool.dex_id, &second_request).await?;
                log_step(db,&sig.id,Some(&order_id),"DEX_SECOND_QUOTE","OK",Some(&serde_json::json!({
                    "direction":direction.name(),
                    "min_out":second.quote.minimum_out_atomic,
                    "expected_out":second.quote.amount_out_atomic,
                    "slot":second.quote.slot
                }).to_string())).await?;
                Ok((first, second))
            }.await;
            match attempt {
                Ok((first, second)) => {
                    let projected_roi = (second.quote.minimum_out_atomic as f64 / initial_amount as f64) - 1.0;
                    if winner.as_ref().map_or(true, |(_,_,_,_,roi)| projected_roi > *roi) {
                        winner = Some((direction, first, second, initial_amount, projected_roi));
                    }
                }
                Err(err) => {
                    failures.push(format!("{}: {err:#}", direction.name()));
                    log_step(db,&sig.id,Some(&order_id),"DIRECTION_ERROR","ERROR",Some(&format!("{}: {err:#}",direction.name()))).await?;
                }
            }
        }

        let (direction, first, second, initial_amount, projected_roi) = winner.ok_or_else(|| anyhow::anyhow!("No viable route: {}",failures.join(" | ")))?;
        let (first_pool, second_pool, first_mint, second_mint) = match direction {
            Direction::BuySell => (&buy_pool, &sell_pool, &cfg.allowed_quote, &token),
            Direction::SellBuy => (&sell_pool, &buy_pool, &token, &cfg.allowed_quote),
            Direction::Auto => unreachable!(),
        };
        let min_out = second.quote.minimum_out_atomic;
        log_step(db,&sig.id,Some(&order_id),"DIRECTION_SELECTED","OK",Some(&serde_json::json!({
            "direction":direction.name(),
            "initial_atomic":initial_amount,
            "minimum_final_atomic":min_out,
            "projected_roi_pct":projected_roi*100.0
        }).to_string())).await?;
        if min_out <= initial_amount && !cfg.simulate_ignore_profit {
            bail!("Worst-case unprofitable: direction={}, input={}, return={}",direction.name(),initial_amount,min_out);
        }
        

        // ATAs da Raydium devem ser criadas antes das instruções de swap.
        // Dedupe por mint: ambas as pernas compartilham a mesma wallet.
        let mut raydium_mints = Vec::new();
        if matches!(dex_router::get_adapter(&first_pool.dex_id)?, dex_router::DexType::RaydiumClmm) {
            raydium_mints.push(Pubkey::from_str(first_mint)?);
            raydium_mints.push(Pubkey::from_str(second_mint)?);
        }
        if matches!(dex_router::get_adapter(&second_pool.dex_id)?, dex_router::DexType::RaydiumClmm) {
            raydium_mints.push(Pubkey::from_str(second_mint)?);
            raydium_mints.push(Pubkey::from_str(first_mint)?);
        }
        let prep = prepare_token_accounts(
            cfg,
            Pubkey::from_str(first_mint)?,
            initial_amount,
            matches!(dex_router::get_adapter(&first_pool.dex_id)?, dex_router::DexType::RaydiumClmm),
            &raydium_mints,
        ).await?;
        let mut first_instructions = prep;
        first_instructions.extend(first.instructions.iter().cloned());


        let lookup_tables = load_lookup_tables(&cfg.rpc).await?;
        let blockhash = cfg.rpc.get_latest_blockhash().await?;
        let additional_signers: Vec<&Keypair> = first.additional_signers.iter().chain(second.additional_signers.iter()).collect();
        log_step(db,&sig.id,Some(&order_id),"ATOMIC_TX_BUILD","STARTED",Some(direction.name())).await?;

        let mut alt_addresses = std::collections::BTreeSet::new();
        for ix in first_instructions.iter().chain(second.instructions.iter()) {
            for account in &ix.accounts {
                if !account.is_signer && account.pubkey != cfg.wallet.pubkey() {
                    alt_addresses.insert(account.pubkey);
                }
            }
        }
        println!("=== ALT ADDRESSES ===");
        for address in &alt_addresses {
            println!("{address}");
        }
        println!("=== END ALT ADDRESSES ===");

        let tx = transaction_builder::atomic_transaction(
            &first_instructions,
            &second.instructions,
            &lookup_tables,
            &cfg.wallet,
            blockhash,
            &additional_signers,
            cfg.compute_limit,
            cfg.compute_price,
        )?;

        log_step(db,&sig.id,Some(&order_id),"ATOMIC_TX_BUILD","OK",Some(&serde_json::json!({
            "direction":direction.name(),
            "signature":tx.signatures.first().map(|v|v.to_string()),
            "first_ixs":first.instructions.len(),
            "second_ixs":second.instructions.len(),
            "additional_signers":additional_signers.len()
        }).to_string())).await?;
        let sim = cfg.rpc.simulate_transaction(&tx).await?;
        let error = sim.value.err.as_ref().map(|e|format!("{e:?}"));
        log_step(db,&sig.id,Some(&order_id),"ATOMIC_SIMULATION",if error.is_some(){"ERROR"}else{"OK"},Some(&serde_json::json!({
            "direction":direction.name(),
            "error":error,
            "logs":sim.value.logs,
            "units_consumed":sim.value.units_consumed,
            "input_atomic":initial_amount,
            "minimum_final_atomic":min_out,
            "sent":false
        }).to_string())).await?;

        if let Some(err) = error { bail!("Atomic simulation failed: {err}"); }
        update(db,&order_id,&sig.id,"SIMULATED",None,None).await?;
        Ok(())

    }.await;
    if let Err(ref e)=result {
        let row = sqlx::query("SELECT status FROM trade_orders WHERE id=?").bind(&order_id).fetch_one(db).await?;
        let current: String = row.try_get("status")?;
        if current=="PROCESSING" {
            update(db,&order_id,&sig.id,"REJECTED",None,Some(&e.to_string())).await?;
        }
        log_step(db,&sig.id,Some(&order_id),"ARBITRAGE_TEST_ERROR","ERROR",Some(&e.to_string())).await?;
    }
    result
}

pub async fn run(db: MySqlPool) -> Result<()> {
    if env::var("ARBITRAGE_LIVE_ENABLED").as_deref()==Ok("true") {
        bail!("LIVE disabled in cross-DEX atomic simulation prototype");
    }

    set_native_mint_wrapping_strategy(
        NativeMintWrappingStrategy::None,
    ).map_err(|e| anyhow::anyhow!("Orca WSOL config: {e}"))?;

    set_enforce_token_balance_check(false)
        .map_err(|e| anyhow::anyhow!("Orca balance config: {e}"))?;

    let cfg=Arc::new(Runtime {
        rpc: RpcClient::new(env::var("SOLANA_RPC_URL").context("SOLANA_RPC_URL required")?),
        wallet: read_keypair_file(env::var("SOLANA_KEYPAIR_PATH").context("SOLANA_KEYPAIR_PATH required")?).map_err(|e|anyhow::anyhow!("Wallet read error: {e}"))?,
        allowed_quote: env::var("ARBITRAGE_ALLOWED_QUOTE_MINT").context("ARBITRAGE_ALLOWED_QUOTE_MINT required")?,
        amount_raw: env::var("ARBITRAGE_AMOUNT_RAW").context("ARBITRAGE_AMOUNT_RAW required")?.parse()?,
        token_amount_raw: config("ARBITRAGE_TOKEN_AMOUNT_RAW", 0_u64),
        direction: Direction::parse(&env::var("ARBITRAGE_DIRECTION").unwrap_or_else(|_| "AUTO".into()))?,
        slippage_bps: config("ARBITRAGE_SLIPPAGE_BPS",30_u16),
        compute_limit: config("ARBITRAGE_COMPUTE_UNIT_LIMIT",1_300_000_u32),
        compute_price: config("ARBITRAGE_CU_PRICE_MICROLAMPORTS",0_u64),
        simulate_ignore_profit: env::var("DEX_TEST_IGNORE_PROFIT").as_deref()==Ok("true") || env::var("ORCA_TEST_IGNORE_PROFIT").as_deref()==Ok("true"),
    });
    if cfg.slippage_bps>=10_000 {bail!("Invalid slippage")}
    if cfg.direction != Direction::SellBuy && cfg.amount_raw == 0 {bail!("ARBITRAGE_AMOUNT_RAW must be positive")}
    if cfg.direction == Direction::SellBuy && cfg.token_amount_raw == 0 {bail!("ARBITRAGE_TOKEN_AMOUNT_RAW must be positive for SELL_BUY")}
    if cfg.compute_limit == 0 || cfg.compute_limit > 1_400_000 { bail!("Invalid compute unit limit"); }
    let mq=channel().await?;
    mq.basic_qos(1,BasicQosOptions::default()).await?;
    let mut stream=mq.basic_consume("signals.arbitrage.execute","cross-dex-atomic-test",BasicConsumeOptions::default(),Default::default()).await?;
    while let Some(delivery)=stream.next().await {
        let d=delivery?;
        let work=async { let sig:Signal=serde_json::from_slice(&d.data)?;process(&db,&cfg,sig).await }.await;
        if let Err(e)=work {eprintln!("DEX ATOMIC TEST ERROR: {e:#}");}
        d.ack(BasicAckOptions::default()).await?;
    }
    Ok(())
}

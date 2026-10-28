use anyhow::{bail, Result};
use solana_sdk::{
    address_lookup_table::AddressLookupTableAccount,
    compute_budget::ComputeBudgetInstruction,
    hash::Hash,
    instruction::Instruction,
    message::{v0, VersionedMessage},
    signature::Keypair,
    signer::Signer,
    transaction::VersionedTransaction,
};

pub fn atomic_transaction(
    buy: &[Instruction],
    sell: &[Instruction],
    lookup_tables: &[AddressLookupTableAccount],
    wallet: &Keypair,
    blockhash: Hash,
    compute_unit_limit: u32,
    compute_unit_price_micro_lamports: u64,
) -> Result<VersionedTransaction> {

    if buy.is_empty() { bail!("BUY instructions empty"); }
    if sell.is_empty() { bail!("SELL instructions empty"); }
    if compute_unit_limit == 0 || compute_unit_limit > 1_400_000 {
        bail!("Invalid compute units");
    }
    let mut instructions = vec![
        ComputeBudgetInstruction::set_compute_unit_limit(compute_unit_limit),
        ComputeBudgetInstruction::set_compute_unit_price(compute_unit_price_micro_lamports),
    ];
    
    instructions.extend_from_slice(buy);
    instructions.extend_from_slice(sell);
    let msg = v0::Message::try_compile(
        &wallet.pubkey(), &instructions, lookup_tables, blockhash,
    )?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(msg), &[wallet])?;
    let size = bincode::serialize(&tx)?.len();
    if size > 1232 { bail!("Transaction too large: {size} bytes"); }
    Ok(tx)
}

pub fn single_swap_transaction(
    instructions: &[Instruction],
    wallet: &Keypair,
    blockhash: Hash,
    compute_unit_limit: u32,
) -> Result<VersionedTransaction> {
    if instructions.is_empty() { bail!("Swap instructions empty"); }
    if compute_unit_limit == 0 || compute_unit_limit > 1_400_000 {
        bail!("Invalid compute units");
    }
    let mut all = vec![ComputeBudgetInstruction::set_compute_unit_limit(compute_unit_limit)];
    all.extend_from_slice(instructions);
    let msg = v0::Message::try_compile(&wallet.pubkey(), &all, &[], blockhash)?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(msg), &[wallet])?;
    let size = bincode::serialize(&tx)?.len();
    if size > 1232 { bail!("Transaction too large: {size} bytes"); }
    Ok(tx)
}
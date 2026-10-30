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
    first_swap: &[Instruction],
    second_swap: &[Instruction],
    lookup_tables: &[AddressLookupTableAccount],
    wallet: &Keypair,
    blockhash: Hash,
    additional_signers: &[&Keypair],
    compute_unit_limit: u32,
    compute_unit_price_micro_lamports: u64,
) -> Result<VersionedTransaction> {
    if first_swap.is_empty() {
        bail!("First swap instructions empty");
    }

    if second_swap.is_empty() {
        bail!("Second swap instructions empty");
    }

    if compute_unit_limit == 0 || compute_unit_limit > 1_400_000 {
        bail!("Invalid compute units");
    }

    let mut instructions = vec![
        ComputeBudgetInstruction::set_compute_unit_limit(compute_unit_limit),
        ComputeBudgetInstruction::set_compute_unit_price(
            compute_unit_price_micro_lamports,
        ),
    ];

    instructions.extend_from_slice(first_swap);
    instructions.extend_from_slice(second_swap);

    let msg = v0::Message::try_compile(
        &wallet.pubkey(),
        &instructions,
        lookup_tables,
        blockhash,
    )?;

    let mut signers: Vec<&Keypair> =
        Vec::with_capacity(1 + additional_signers.len());

    signers.push(wallet);
    signers.extend_from_slice(additional_signers);

    let tx = VersionedTransaction::try_new(
        VersionedMessage::V0(msg),
        &signers,
    )?;

    let size = bincode::serialize(&tx)?.len();

    if size > 1232 {
        bail!("Transaction too large: {size} bytes");
    }

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
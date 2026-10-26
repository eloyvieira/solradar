use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use solana_sdk::{
    address_lookup_table::AddressLookupTableAccount,
    compute_budget::ComputeBudgetInstruction,
    hash::Hash,
    instruction::{AccountMeta, Instruction},
    message::{v0, VersionedMessage},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::VersionedTransaction,
};
use std::{collections::HashMap, str::FromStr};
use crate::jupiter_v2::{ApiInstruction, BuildResponse};

fn to_instruction(ix: &ApiInstruction) -> Result<Instruction> {
    let keys = ix.accounts.iter().map(|a| {
        let key = Pubkey::from_str(&a.pubkey)?;
        Ok(if a.is_writable { AccountMeta::new(key, a.is_signer) }
           else { AccountMeta::new_readonly(key, a.is_signer) })
    }).collect::<Result<Vec<_>>>()?;
    Ok(Instruction {
        program_id: Pubkey::from_str(&ix.program_id)?,
        accounts: keys,
        data: STANDARD.decode(&ix.data).context("Jupiter Instruction: invalid base64")?,
    })
}

pub fn atomic_transaction(
    buy: &BuildResponse,
    sell: &BuildResponse,
    wallet: &Keypair,
    blockhash: Hash,
    compute_unit_limit: u32,
    compute_unit_price_micro_lamports: u64,
) -> Result<VersionedTransaction> {

    if buy.cleanup_instruction.is_some() || sell.cleanup_instruction.is_some() {
        bail!("Route with cleanupInstruction: composition not yet validated; rejected");
    }
    if !buy.other_instructions.is_empty() || !sell.other_instructions.is_empty() {
        bail!("Route with otherInstructions: composition not yet validated; rejected");
    }
    if compute_unit_limit == 0 || compute_unit_limit > 1_400_000 {
        bail!("Compute units invalid");
    }
    let mut ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_limit(compute_unit_limit),
        ComputeBudgetInstruction::set_compute_unit_price(compute_unit_price_micro_lamports),
    ];
    for ix in &buy.setup_instructions { ixs.push(to_instruction(ix)?); }
    ixs.push(to_instruction(&buy.swap_instruction)?);
    for ix in &sell.setup_instructions { ixs.push(to_instruction(ix)?); }
    ixs.push(to_instruction(&sell.swap_instruction)?);

    let mut tables: HashMap<String, Vec<String>> = HashMap::new();
    for response in [buy, sell] {
        if let Some(addrs) = &response.addresses_by_lookup_table_address {
            for (key, addresses) in addrs {
                if let Some(existing) = tables.get(key) {
                    if existing != addresses { bail!("Lookup table contraditória: {}", key); }
                } else { tables.insert(key.clone(), addresses.clone()); }
            }
        }
    }
    let lookup_tables = tables.into_iter().map(|(key, addresses)| {
        Ok(AddressLookupTableAccount {
            key: Pubkey::from_str(&key)?,
            addresses: addresses.iter().map(|s| Pubkey::from_str(s))
                .collect::<std::result::Result<Vec<_>, _>>()?,
        })
    }).collect::<Result<Vec<_>>>()?;
    let msg = v0::Message::try_compile(&wallet.pubkey(), &ixs, &lookup_tables, blockhash)?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(msg), &[wallet])?;

    let wire = bincode::serialize(&tx)?;
    if wire.len() > 1232 { bail!("v0 transaction too large: {} bytes", wire.len()); }
    Ok(tx)
}

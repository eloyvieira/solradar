use anyhow::{bail, Result};
use solana_sdk::{
    address_lookup_table::AddressLookupTableAccount,
    hash::Hash,
    instruction::Instruction,
    message::{v0, VersionedMessage},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::VersionedTransaction,
};

#[derive(Debug)]
pub struct SwapInstructions {
    pub instructions: Vec<Instruction>,
    pub lookup_tables: Vec<AddressLookupTableAccount>,
}

pub struct AtomicArbitrageBuilder {
    buy: SwapInstructions,
    sell: SwapInstructions,
}

impl AtomicArbitrageBuilder {
    pub fn new(
        buy: SwapInstructions,
        sell: SwapInstructions,
    ) -> Result<Self> {
        if buy.instructions.is_empty() {
            bail!("Swap buy without instructions");
        }

        if sell.instructions.is_empty() {
            bail!("Swap sell without instructions");
        }
        Ok(Self { buy, sell })
    }

    pub fn build(
        self,
        payer: &Keypair,
        recent_blockhash: Hash,
    ) -> Result<VersionedTransaction> {
        let mut instructions = Vec::new();

        // mandatory order:
        // first buy, then sell.
        instructions.extend(self.buy.instructions);
        instructions.extend(self.sell.instructions);

        // consolidate address lookup tables
        let mut lookup_tables = self.buy.lookup_tables;
        for table in self.sell.lookup_tables {
            if !lookup_tables
                .iter()
                .any(|existing| existing.key == table.key)
            {
                lookup_tables.push(table);
            }
        }

        let payer_pubkey: Pubkey = payer.pubkey();
        let message = v0::Message::try_compile(
            &payer_pubkey,
            &instructions,
            &lookup_tables,
            recent_blockhash,
        )?;
        let transaction = VersionedTransaction::try_new(
            VersionedMessage::V0(message),
            &[payer],
        )?;

        Ok(transaction)
    }
}

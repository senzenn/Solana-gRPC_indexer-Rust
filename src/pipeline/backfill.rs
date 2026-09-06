use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::RpcBlockConfig;
use solana_transaction_status::option_serializer::OptionSerializer;
use solana_transaction_status::{
    EncodedTransaction, EncodedTransactionWithStatusMeta, TransactionDetails, UiCompiledInstruction,
    UiInstruction, UiMessage, UiParsedInstruction, UiTransactionEncoding, UiTransactionStatusMeta,
};
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::event::RawInstruction;
use super::idl::IdlRouter;
use super::ingest::decode_instructions;
use super::store::EventStore;

const BACKFILL_RATE: Duration = Duration::from_millis(200); // 5 req/s

pub struct BackfillWorker {
    rpc_url: String,
    store: Arc<EventStore>,
    router: IdlRouter,
    track_tokens: bool,
}

impl BackfillWorker {
    pub fn new(
        rpc_url: String,
        store: Arc<EventStore>,
        router: IdlRouter,
        track_tokens: bool,
    ) -> Self {
        Self {
            rpc_url,
            store,
            router,
            track_tokens,
        }
    }

    pub async fn run(self, mut gap_rx: mpsc::Receiver<(u64, u64)>) -> Result<()> {
        let client = RpcClient::new(self.rpc_url.clone());
        let block_config = RpcBlockConfig {
            encoding: Some(UiTransactionEncoding::Json),
            transaction_details: Some(TransactionDetails::Full),
            rewards: Some(false),
            commitment: None,
            max_supported_transaction_version: Some(0),
        };

        while let Some((from, to)) = gap_rx.recv().await {
            info!(from, to, "backfill gap started");
            for slot in from..=to {
                tokio::time::sleep(BACKFILL_RATE).await;
                match client.get_block_with_config(slot, block_config).await {
                    Ok(block) => {
                        let mut ixs = Vec::new();
                        if let Some(txs) = block.transactions {
                            for tx in txs {
                                if tx_failed(&tx) {
                                    continue;
                                }
                                ixs.extend(extract_instructions(slot, &tx));
                            }
                        }
                        let events =
                            decode_instructions(&self.router, &ixs, self.track_tokens);
                        let event_count = events.len();
                        for event in events {
                            let _ = self.store.write_event(&event).await;
                        }
                        info!(slot, instructions = ixs.len(), events = event_count, "backfilled slot");
                    }
                    Err(err) => {
                        warn!(slot, error = %err, "backfill slot fetch failed");
                    }
                }
            }
            self.store.mark_gap_filled(from, to).await?;
            info!(from, to, "backfill gap complete");
        }
        Ok(())
    }
}

fn tx_failed(tx: &EncodedTransactionWithStatusMeta) -> bool {
    tx.meta.as_ref().is_some_and(|meta| meta.err.is_some())
}

fn extract_instructions(slot: u64, tx: &EncodedTransactionWithStatusMeta) -> Vec<RawInstruction> {
    let EncodedTransaction::Json(ui_tx) = &tx.transaction else {
        return Vec::new();
    };
    let signature = ui_tx
        .signatures
        .first()
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());
    let keys = account_keys(&ui_tx.message, tx.meta.as_ref());
    let mut out = Vec::new();
    let mut ix_index = 0u32;

    match &ui_tx.message {
        UiMessage::Raw(raw) => {
            for ix in &raw.instructions {
                if let Some(raw_ix) =
                    compiled_ui_to_raw(slot, &signature, ix_index, &keys, ix)
                {
                    out.push(raw_ix);
                }
                ix_index += 1;
            }
        }
        UiMessage::Parsed(parsed) => {
            for ui_ix in &parsed.instructions {
                if let Some(raw_ix) = ui_instruction_to_raw(slot, &signature, ix_index, &keys, ui_ix)
                {
                    out.push(raw_ix);
                }
                ix_index += 1;
            }
        }
    }

    if let Some(meta) = tx.meta.as_ref() {
        let inner: Option<Vec<_>> = meta.inner_instructions.clone().into();
        if let Some(groups) = inner {
            for group in groups {
                for ui_ix in group.instructions {
                    if let Some(raw_ix) =
                        ui_instruction_to_raw(slot, &signature, ix_index, &keys, &ui_ix)
                    {
                        out.push(raw_ix);
                    }
                    ix_index += 1;
                }
            }
        }
    }

    out
}

fn account_keys(message: &UiMessage, meta: Option<&UiTransactionStatusMeta>) -> Vec<String> {
    let mut keys = match message {
        UiMessage::Raw(raw) => raw.account_keys.clone(),
        UiMessage::Parsed(parsed) => parsed
            .account_keys
            .iter()
            .map(|account| account.pubkey.clone())
            .collect(),
    };

    if let Some(meta) = meta
        && let OptionSerializer::Some(loaded) = &meta.loaded_addresses {
            keys.extend(loaded.writable.iter().cloned());
            keys.extend(loaded.readonly.iter().cloned());
        }

    keys
}

fn compiled_ui_to_raw(
    slot: u64,
    signature: &str,
    ix_index: u32,
    keys: &[String],
    ix: &UiCompiledInstruction,
) -> Option<RawInstruction> {
    let program_id = keys.get(ix.program_id_index as usize)?.clone();
    let accounts = ix
        .accounts
        .iter()
        .filter_map(|idx| keys.get(*idx as usize).cloned())
        .collect();
    let data = bs58::decode(&ix.data).into_vec().ok()?;
    Some(RawInstruction {
        slot,
        signature: signature.to_string(),
        ix_index,
        program_id,
        accounts,
        data,
    })
}

fn ui_instruction_to_raw(
    slot: u64,
    signature: &str,
    ix_index: u32,
    keys: &[String],
    ix: &UiInstruction,
) -> Option<RawInstruction> {
    match ix {
        UiInstruction::Compiled(compiled) => {
            compiled_ui_to_raw(slot, signature, ix_index, keys, compiled)
        }
        UiInstruction::Parsed(UiParsedInstruction::PartiallyDecoded(partial)) => {
            let data = bs58::decode(&partial.data).into_vec().ok()?;
            Some(RawInstruction {
                slot,
                signature: signature.to_string(),
                ix_index,
                program_id: partial.program_id.clone(),
                accounts: partial.accounts.clone(),
                data,
            })
        }
        UiInstruction::Parsed(UiParsedInstruction::Parsed(_)) => None,
    }
}

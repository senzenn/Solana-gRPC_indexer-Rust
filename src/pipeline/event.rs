use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use tracing::warn;

use super::ix_path::IxPath;

pub const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
pub const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IndexEvent {
    Slot {
        slot: u64,
        parent: Option<u64>,
    },
    TokenTransfer {
        slot: u64,
        signature: String,
        ix_index: u32,
        #[serde(default)]
        ix_path: String,
        program: String,
        source: String,
        destination: String,
        authority: Option<String>,
        mint: Option<String>,
        amount: u64,
        decimals: Option<u8>,
    },
    ProgramIx {
        slot: u64,
        signature: String,
        ix_index: u32,
        #[serde(default)]
        ix_path: String,
        program_id: String,
        program_name: String,
        instruction: String,
        accounts: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        args: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        named_accounts: Option<Vec<NamedAccount>>,
        /// True for an instruction of a failed transaction. Left out when false, so
        /// payloads of successful transactions do not change.
        #[serde(default, skip_serializing_if = "is_false")]
        failed: bool,
    },
    IdlDrift {
        program_id: String,
        program_name: String,
        severity: String,
        summary: String,
    },
    Gap {
        from_slot: u64,
        to_slot: u64,
    },
}

impl IndexEvent {
    pub fn slot(&self) -> Option<u64> {
        match self {
            Self::Slot { slot, .. }
            | Self::TokenTransfer { slot, .. }
            | Self::ProgramIx { slot, .. } => Some(*slot),
            Self::Gap { to_slot, .. } => Some(*to_slot),
            Self::IdlDrift { .. } => None,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Slot { .. } => "slot",
            Self::TokenTransfer { .. } => "token_transfer",
            Self::ProgramIx { .. } => "program_ix",
            Self::IdlDrift { .. } => "idl_drift",
            Self::Gap { .. } => "gap",
        }
    }

    pub fn program(&self) -> &str {
        match self {
            Self::TokenTransfer { program, .. } => program,
            Self::ProgramIx { program_name, .. } => program_name,
            Self::IdlDrift { program_name, .. } => program_name,
            Self::Slot { .. } => "slot",
            Self::Gap { .. } => "gap",
        }
    }

    #[cfg(test)]
    pub fn signature(&self) -> Option<&str> {
        match self {
            Self::TokenTransfer { signature, .. } | Self::ProgramIx { signature, .. } => {
                Some(signature)
            }
            _ => None,
        }
    }

    pub fn ix_index(&self) -> Option<u32> {
        match self {
            Self::TokenTransfer { ix_index, .. } | Self::ProgramIx { ix_index, .. } => {
                Some(*ix_index)
            }
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn ix_path(&self) -> Option<&str> {
        match self {
            Self::TokenTransfer { ix_path, .. } | Self::ProgramIx { ix_path, .. } => {
                Some(ix_path.as_str())
            }
            _ => None,
        }
    }

    /// `(signature, ix_path)` used as the store unique key.
    pub fn store_identity(&self) -> (String, String) {
        match self {
            Self::TokenTransfer {
                signature, ix_path, ..
            }
            | Self::ProgramIx {
                signature, ix_path, ..
            } => (signature.clone(), ix_path.clone()),
            Self::Gap {
                from_slot, to_slot, ..
            } => (String::new(), format!("gap:{from_slot}:{to_slot}")),
            Self::Slot { slot, .. } => (String::new(), format!("slot:{slot}")),
            Self::IdlDrift { program_id, .. } => (String::new(), format!("drift:{program_id}")),
        }
    }

    pub fn fill_ix_path(&mut self) {
        match self {
            Self::TokenTransfer {
                ix_index, ix_path, ..
            }
            | Self::ProgramIx {
                ix_index, ix_path, ..
            } if ix_path.is_empty() => {
                *ix_path = ix_index.to_string();
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NamedAccount {
    pub name: String,
    pub pubkey: String,
    /// From the IDL. None for a compiled parser, whose output has no writable flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writable: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct RawInstruction {
    pub slot: u64,
    pub signature: String,
    pub path: IxPath,
    pub program_id: String,
    pub accounts: Vec<String>,
    pub data: Vec<u8>,
    /// The transaction failed. Set by `failed_tx_policy`.
    pub failed: bool,
}

impl RawInstruction {
    pub fn ix_index(&self) -> u32 {
        self.path.outer()
    }

    pub fn ix_path(&self) -> String {
        self.path.to_string()
    }
}

const SIGNATURE_LEN: usize = 64;
/// Longest base58 text of 64 bytes. Longer input is rejected before decode.
const SIGNATURE_B58_MAX: usize = 88;

/// Base58 signature, or None when it is not 64 bytes. A bad endpoint could send
/// any size, and `signature` is in a unique store index.
pub fn signature_from_bytes(raw: &[u8], slot: u64) -> Option<String> {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if raw.len() == SIGNATURE_LEN {
        return Some(bs58::encode(raw).into_string());
    }
    if !WARNED.swap(true, Ordering::Relaxed) {
        warn!(
            slot,
            len = raw.len(),
            "transaction signature is not 64 bytes; transaction dropped (logged once)"
        );
    }
    None
}

/// Same check for a base58 signature from RPC.
pub fn signature_from_base58(signature: &str, slot: u64) -> Option<String> {
    let raw = if signature.len() <= SIGNATURE_B58_MAX {
        bs58::decode(signature).into_vec().unwrap_or_default()
    } else {
        Vec::new()
    };
    signature_from_bytes(&raw, slot)
}

/// Account keys of one instruction. A bad index drops the whole instruction:
/// skipping only that account would move every later account to a wrong role.
pub fn resolve_accounts(
    keys: &[String],
    indexes: &[u8],
    slot: u64,
    signature: &str,
) -> Option<Vec<String>> {
    static WARNED: AtomicBool = AtomicBool::new(false);
    let accounts: Option<Vec<String>> = indexes
        .iter()
        .map(|idx| keys.get(usize::from(*idx)).cloned())
        .collect();
    if accounts.is_none() && !WARNED.swap(true, Ordering::Relaxed) {
        warn!(
            slot,
            signature, "instruction account index out of range; instruction dropped (logged once)"
        );
    }
    accounts
}

/// Instructions of one transaction to index. A failed transaction moves no
/// tokens, so its token-program instructions never become transfers.
pub fn failed_tx_policy(
    ixs: Vec<RawInstruction>,
    failed: bool,
    track_failed: bool,
) -> Vec<RawInstruction> {
    if !failed {
        return ixs;
    }
    if !track_failed {
        return Vec::new();
    }
    ixs.into_iter()
        .filter(|ix| ix.program_id != TOKEN_PROGRAM && ix.program_id != TOKEN_2022_PROGRAM)
        .map(|ix| RawInstruction { failed: true, ..ix })
        .collect()
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_ix_omits_empty_optional_fields() {
        let event = IndexEvent::ProgramIx {
            slot: 1,
            signature: "sig".into(),
            ix_index: 0,
            ix_path: "0".into(),
            program_id: "p".into(),
            program_name: "demo".into(),
            instruction: "swap".into(),
            accounts: vec![],
            args: None,
            named_accounts: None,
            failed: false,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert!(json.get("args").is_none());
        assert!(json.get("named_accounts").is_none());
    }

    #[test]
    fn token_transfer_kind_is_stable() {
        let event = IndexEvent::TokenTransfer {
            slot: 1,
            signature: "sig".into(),
            ix_index: 0,
            ix_path: "0".into(),
            program: TOKEN_PROGRAM.into(),
            source: "a".into(),
            destination: "b".into(),
            authority: None,
            mint: None,
            amount: 7,
            decimals: None,
        };
        assert_eq!(event.kind(), "token_transfer");
        assert_eq!(event.slot(), Some(1));
        assert_eq!(event.store_identity(), ("sig".into(), "0".into()));
    }

    #[test]
    fn fill_ix_path_from_index_when_empty() {
        let mut event = IndexEvent::ProgramIx {
            slot: 1,
            signature: "s".into(),
            ix_index: 4,
            ix_path: String::new(),
            program_id: "p".into(),
            program_name: "demo".into(),
            instruction: "swap".into(),
            accounts: vec![],
            args: None,
            named_accounts: None,
            failed: false,
        };
        event.fill_ix_path();
        assert_eq!(event.ix_path(), Some("4"));
    }

    #[test]
    fn gap_identity_is_range() {
        let event = IndexEvent::Gap {
            from_slot: 10,
            to_slot: 12,
        };
        assert_eq!(event.store_identity(), (String::new(), "gap:10:12".into()));
    }
}

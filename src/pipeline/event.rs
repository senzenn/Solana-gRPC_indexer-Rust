use serde::{Deserialize, Serialize};

pub const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
pub const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
#[allow(dead_code)]
pub const PUMP_FUN_PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

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
        program_id: String,
        program_name: String,
        instruction: String,
        accounts: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        args: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        named_accounts: Option<Vec<NamedAccount>>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NamedAccount {
    pub name: String,
    pub pubkey: String,
    pub writable: bool,
}

#[derive(Debug, Clone)]
pub struct RawInstruction {
    pub slot: u64,
    pub signature: String,
    pub ix_index: u32,
    pub program_id: String,
    pub accounts: Vec<String>,
    pub data: Vec<u8>,
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
            program_id: "p".into(),
            program_name: "demo".into(),
            instruction: "swap".into(),
            accounts: vec![],
            args: None,
            named_accounts: None,
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
    }
}

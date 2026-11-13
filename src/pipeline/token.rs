use serde_json::{Value, json};
use solana_sdk::message::{AccountKeys, compiled_instruction::CompiledInstruction};
use solana_sdk::pubkey::Pubkey;
use solana_transaction_status::parse_token::parse_token;

use super::event::{IndexEvent, RawInstruction};

const TRANSFER: u8 = 3;
const TRANSFER_CHECKED: u8 = 12;

/// Decodes one SPL Token or Token-2022 instruction. `only` holds the snake_case
/// names to keep (`transfer`, `mint_to`, ...); an empty list keeps all.
pub fn parse_token_ix(ix: &RawInstruction, only: &[String]) -> Option<IndexEvent> {
    let wanted = |name: &str| only.is_empty() || only.iter().any(|n| n == name);
    if let Some((name, event)) = transfer(ix) {
        return wanted(name).then_some(event);
    }
    let (instruction, info) = decode(ix);
    wanted(&instruction).then(|| IndexEvent::TokenIx {
        slot: ix.slot,
        signature: ix.signature.clone(),
        ix_index: ix.ix_index(),
        ix_path: ix.ix_path(),
        program: ix.program_id.clone(),
        instruction,
        info,
    })
}

fn transfer(ix: &RawInstruction) -> Option<(&'static str, IndexEvent)> {
    let checked = match *ix.data.first()? {
        TRANSFER if ix.data.len() >= 9 && ix.accounts.len() >= 2 => false,
        TRANSFER_CHECKED if ix.data.len() >= 10 && ix.accounts.len() >= 3 => true,
        _ => return None,
    };
    let amount = u64::from_le_bytes(ix.data[1..9].try_into().ok()?);
    // Transfer: source, destination, authority. TransferChecked: source, mint, destination, authority.
    let (destination, authority, mint, decimals) = if checked {
        (2, 3, ix.accounts.get(1).cloned(), Some(ix.data[9]))
    } else {
        (1, 2, None, None)
    };
    let event = IndexEvent::TokenTransfer {
        slot: ix.slot,
        signature: ix.signature.clone(),
        ix_index: ix.ix_index(),
        ix_path: ix.ix_path(),
        program: ix.program_id.clone(),
        source: ix.accounts[0].clone(),
        destination: ix.accounts[destination].clone(),
        authority: ix.accounts.get(authority).cloned(),
        mint,
        amount,
        decimals,
    };
    Some((
        if checked {
            "transfer_checked"
        } else {
            "transfer"
        },
        event,
    ))
}

/// Uses the RPC `jsonParsed` decoder. Data it cannot read (for example p-token
/// tags 45 and 255) becomes `unknown` with the raw bytes, so nothing is lost.
fn decode(ix: &RawInstruction) -> (String, Value) {
    match parsed(ix) {
        Some((name, info)) => (snake_case(&name), snake_keys(info)),
        None => (
            "unknown".into(),
            json!({
                "tag": ix.data.first(),
                "data": bs58::encode(&ix.data).into_string(),
                "accounts": ix.accounts,
            }),
        ),
    }
}

fn parsed(ix: &RawInstruction) -> Option<(String, Value)> {
    let keys = ix
        .accounts
        .iter()
        .map(|key| key.parse::<Pubkey>().ok())
        .collect::<Option<Vec<_>>>()?;
    // The decoder reads accounts as u8 indexes into the key list.
    let accounts = (0..keys.len())
        .map(|i| u8::try_from(i).ok())
        .collect::<Option<Vec<_>>>()?;
    let compiled = CompiledInstruction {
        program_id_index: 0,
        accounts,
        data: ix.data.clone(),
    };
    let parsed = parse_token(&compiled, &AccountKeys::new(&keys, None)).ok()?;
    Some((parsed.instruction_type, parsed.info))
}

/// `transferCheckedWithFee` -> `transfer_checked_with_fee`, `initializeMint2` -> `initialize_mint2`.
fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            if !out.is_empty() {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn snake_keys(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (snake_case(&key), snake_keys(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(snake_keys).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::event::{TOKEN_2022_PROGRAM, TOKEN_PROGRAM};

    fn ix(program: &str, accounts: &[&str], data: Vec<u8>) -> RawInstruction {
        RawInstruction {
            slot: 42,
            signature: "sig".into(),
            path: crate::pipeline::ix_path::IxPath::root(1),
            program_id: program.into(),
            accounts: accounts.iter().map(|s| (*s).to_string()).collect(),
            data,
            failed: false,
        }
    }

    fn keys(n: usize) -> Vec<String> {
        (0..n).map(|_| Pubkey::new_unique().to_string()).collect()
    }

    fn token_ix(event: Option<IndexEvent>) -> (String, Value) {
        match event {
            Some(IndexEvent::TokenIx {
                instruction, info, ..
            }) => (instruction, info),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_spl_token_transfer() {
        let mut data = vec![TRANSFER];
        data.extend_from_slice(&1_000u64.to_le_bytes());
        let event = parse_token_ix(&ix(TOKEN_PROGRAM, &["src", "dst", "auth"], data), &[]);
        match event.unwrap() {
            IndexEvent::TokenTransfer {
                amount,
                source,
                destination,
                ..
            } => {
                assert_eq!(amount, 1_000);
                assert_eq!(source, "src");
                assert_eq!(destination, "dst");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_transfer_checked_with_mint() {
        let mut data = vec![TRANSFER_CHECKED];
        data.extend_from_slice(&50u64.to_le_bytes());
        data.push(6);
        let event = parse_token_ix(
            &ix(TOKEN_2022_PROGRAM, &["src", "mint", "dst", "auth"], data),
            &[],
        );
        match event.unwrap() {
            IndexEvent::TokenTransfer {
                amount,
                decimals,
                mint,
                destination,
                ..
            } => {
                assert_eq!(amount, 50);
                assert_eq!(decimals, Some(6));
                assert_eq!(mint.as_deref(), Some("mint"));
                assert_eq!(destination, "dst");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn mint_to_checked_has_snake_case_name_and_keys() {
        let accounts = keys(3);
        let refs: Vec<&str> = accounts.iter().map(String::as_str).collect();
        let mut data = vec![14];
        data.extend_from_slice(&2_500u64.to_le_bytes());
        data.push(3);
        let (name, info) = token_ix(parse_token_ix(&ix(TOKEN_PROGRAM, &refs, data), &[]));
        assert_eq!(name, "mint_to_checked");
        assert_eq!(info["mint"], accounts[0]);
        assert_eq!(info["mint_authority"], accounts[2]);
        assert_eq!(info["token_amount"]["ui_amount_string"], "2.5");
    }

    #[test]
    fn token_2022_extension_is_decoded() {
        // TransferFeeExtension (26), sub-instruction TransferCheckedWithFee (1).
        let accounts = keys(4);
        let refs: Vec<&str> = accounts.iter().map(String::as_str).collect();
        let mut data = vec![26, 1];
        data.extend_from_slice(&1_000u64.to_le_bytes());
        data.push(2);
        data.extend_from_slice(&10u64.to_le_bytes());
        let (name, info) = token_ix(parse_token_ix(&ix(TOKEN_2022_PROGRAM, &refs, data), &[]));
        assert_eq!(name, "transfer_checked_with_fee");
        assert_eq!(info["fee_amount"]["amount"], "10");
    }

    #[test]
    fn unreadable_data_is_kept_as_unknown() {
        // p-token UnwrapLamports (45) from mainnet; the decoder predates it.
        let accounts = keys(3);
        let refs: Vec<&str> = accounts.iter().map(String::as_str).collect();
        let data = vec![45, 1, 0xf3, 0xe3, 0x4c, 0, 0, 0, 0, 0];
        let (name, info) = token_ix(parse_token_ix(&ix(TOKEN_PROGRAM, &refs, data.clone()), &[]));
        assert_eq!(name, "unknown");
        assert_eq!(info["tag"], 45);
        assert_eq!(info["data"], bs58::encode(&data).into_string());
        assert_eq!(info["accounts"][2], accounts[2]);
    }

    #[test]
    fn only_list_drops_other_instructions() {
        let accounts = keys(3);
        let refs: Vec<&str> = accounts.iter().map(String::as_str).collect();
        let only = vec!["burn".to_string()];
        let mut transfer = vec![TRANSFER];
        transfer.extend_from_slice(&1u64.to_le_bytes());
        assert!(parse_token_ix(&ix(TOKEN_PROGRAM, &refs, transfer), &only).is_none());
        let mut burn = vec![8];
        burn.extend_from_slice(&1u64.to_le_bytes());
        let (name, _) = token_ix(parse_token_ix(&ix(TOKEN_PROGRAM, &refs, burn), &only));
        assert_eq!(name, "burn");
    }
}

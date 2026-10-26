//! Typed parsers that shipstern generates at build time from the IDLs in
//! `parsers/`. build.rs writes the `include_shipstern_parser!` calls.
//!
//! An IDL file in the runtime IDL dir wins over a compiled parser for the same
//! program (see `IdlRouter::parse`).

use std::collections::HashMap;
use std::io;
use std::str::FromStr;
use std::sync::{Arc, LazyLock};

use futures::FutureExt;
use serde::Serialize;
use serde_json::Value;
use shipstern_core::instruction::{InstructionShared, InstructionUpdate, Path};
use shipstern_core::{ParseError, Parser, Pubkey};
use tracing::{debug, warn};

use super::event::{IndexEvent, NamedAccount, RawInstruction};

struct Entry {
    /// Program name from the IDL.
    name: &'static str,
    program_id: [u8; 32],
    parse: fn(&InstructionUpdate) -> Option<Value>,
}

#[allow(dead_code, unused_imports, clippy::all)]
mod generated {
    use super::{Entry, run};

    include!(concat!(env!("OUT_DIR"), "/compiled_parsers.rs"));
}

static PARSERS: LazyLock<HashMap<String, &'static Entry>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    for entry in generated::ENTRIES {
        let id = bs58::encode(entry.program_id).into_string();
        if map.contains_key(&id) {
            warn!(program = %id, name = entry.name, "skip compiled parser: program has one already");
            continue;
        }
        map.insert(id, entry);
    }
    map
});

/// Program ids that have a compiled parser.
pub fn program_ids() -> Vec<String> {
    PARSERS.keys().cloned().collect()
}

pub fn parse(ix: &RawInstruction) -> Option<IndexEvent> {
    parse_with(PARSERS.get(&ix.program_id)?, ix)
}

fn parse_with(entry: &Entry, ix: &RawInstruction) -> Option<IndexEvent> {
    let update = InstructionUpdate {
        program: Pubkey::new(entry.program_id),
        accounts: ix
            .accounts
            .iter()
            .map(|key| pubkey(key))
            .collect::<Option<_>>()?,
        data: ix.data.clone(),
        shared: Arc::new(InstructionShared::default()),
        inner: Vec::new(),
        path: Path::from(ix.path.indices().to_vec()),
        log_range: 0..0,
    };
    // Shape: {"instruction": {"<Variant>": {"accounts": {..}, "args": {..}}}}.
    // A unit variant serializes as the bare name.
    let (instruction, body) = match (entry.parse)(&update)?.get_mut("instruction")?.take() {
        Value::String(name) => (name, Value::Null),
        Value::Object(map) if map.len() == 1 => map.into_iter().next()?,
        _ => return None,
    };
    let args = body
        .get("args")
        .filter(|args| args.as_object().is_some_and(|map| !map.is_empty()))
        .cloned();
    let named_accounts = body
        .get("accounts")
        .and_then(Value::as_object)
        .map(|map| {
            // Optional accounts are null and remaining_accounts is a list; neither has one key.
            map.iter()
                .filter_map(|(name, key)| {
                    Some(NamedAccount {
                        name: name.clone(),
                        pubkey: key.as_str()?.to_string(),
                        writable: None,
                    })
                })
                .collect::<Vec<_>>()
        })
        .filter(|named| !named.is_empty());

    Some(IndexEvent::ProgramIx {
        slot: ix.slot,
        signature: ix.signature.clone(),
        ix_index: ix.ix_index(),
        ix_path: ix.ix_path(),
        program_id: ix.program_id.clone(),
        program_name: entry.name.to_string(),
        instruction,
        accounts: ix.accounts.clone(),
        args,
        named_accounts,
        failed: ix.failed,
    })
}

fn pubkey(key: &str) -> Option<Pubkey> {
    let key = solana_sdk::pubkey::Pubkey::from_str(key).ok()?;
    Some(Pubkey::new(key.to_bytes()))
}

fn run<P>(parser: &P, update: &InstructionUpdate) -> Option<Value>
where
    P: Parser<Input = InstructionUpdate>,
    P::Output: Serialize,
{
    // Generated parsers do no I/O, so the future is ready on the first poll.
    match parser.parse(update).now_or_never()? {
        Ok(output) => to_json(&output),
        Err(ParseError::Filtered | ParseError::DiscriminatorNotFound(_)) => None,
        Err(err) => {
            debug!(parser = %parser.id(), error = %err, "compiled parser failed");
            None
        }
    }
}

/// u128 and i128 go out as strings, the same as the IDL decoder.
/// serde_json::to_value rejects a u128 above u64::MAX.
struct WideIntsAsText;

impl serde_json::ser::Formatter for WideIntsAsText {
    fn write_u128<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: u128) -> io::Result<()> {
        write!(writer, "\"{value}\"")
    }

    fn write_i128<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: i128) -> io::Result<()> {
        write!(writer, "\"{value}\"")
    }
}

fn to_json<T: Serialize>(value: &T) -> Option<Value> {
    let mut buf = Vec::new();
    value
        .serialize(&mut serde_json::Serializer::with_formatter(
            &mut buf,
            WideIntsAsText,
        ))
        .ok()?;
    serde_json::from_slice(&buf).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::ix_path::IxPath;

    // pumpFun.json is the fixture shipstern tests its own macro with.
    #[allow(dead_code, clippy::all)]
    mod sample {
        shipstern_proc_macro::include_shipstern_parser!("idls/pumpFun.json");
    }

    #[test]
    fn sell_becomes_program_ix() {
        let entry = Entry {
            name: "pumpFun",
            program_id: sample::pump_fun::PROGRAM_ID,
            parse: |update| run(&sample::pump_fun::InstructionParser, update),
        };
        let mut data = sample::pump_fun::Instructions::SELL_DISCRIMINATOR.to_vec();
        data.extend_from_slice(&3_878_351_170_692u64.to_le_bytes());
        data.extend_from_slice(&522_403_143u64.to_le_bytes());
        let accounts: Vec<String> = (1..=14u8)
            .map(|i| bs58::encode([i; 32]).into_string())
            .collect();
        let ix = RawInstruction {
            slot: 7,
            signature: "sig".into(),
            path: IxPath::root(2),
            program_id: bs58::encode(entry.program_id).into_string(),
            accounts: accounts.clone(),
            data,
            failed: false,
        };

        let Some(IndexEvent::ProgramIx {
            program_name,
            instruction,
            args,
            named_accounts,
            ..
        }) = parse_with(&entry, &ix)
        else {
            panic!("expected a ProgramIx");
        };
        assert_eq!(program_name, "pumpFun");
        assert_eq!(instruction, "Sell");
        assert_eq!(
            args,
            Some(
                serde_json::json!({"amount": 3_878_351_170_692u64, "min_sol_output": 522_403_143})
            )
        );
        let named = named_accounts.expect("named accounts");
        // user is the 7th account of Sell.
        assert!(named.contains(&NamedAccount {
            name: "user".into(),
            pubkey: accounts[6].clone(),
            writable: None,
        }));
    }

    #[test]
    fn wide_ints_are_text() {
        assert_eq!(
            to_json(&u128::MAX),
            Some(Value::String(u128::MAX.to_string()))
        );
        assert_eq!(
            to_json(&i128::MIN),
            Some(Value::String(i128::MIN.to_string()))
        );
        assert_eq!(to_json(&u64::MAX), Some(serde_json::json!(u64::MAX)));
    }
}

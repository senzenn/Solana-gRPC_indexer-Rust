use std::io::Read;
use std::str::FromStr;

use anyhow::{Context, Result};
use flate2::read::ZlibDecoder;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;

const IDL_SEED: &str = "anchor:idl";

/// Derive the Anchor on-chain IDL account address for a program.
pub fn anchor_idl_address(program_id: &Pubkey) -> Result<Pubkey> {
    let (program_signer, _) = Pubkey::try_find_program_address(&[], program_id)
        .ok_or_else(|| anyhow::anyhow!("failed to derive program signer for {program_id}"))?;
    Pubkey::create_with_seed(&program_signer, IDL_SEED, program_id)
        .with_context(|| format!("create_with_seed for program {program_id}"))
}

/// Fetch and decompress an Anchor IDL JSON string from chain.
pub async fn fetch_idl_json(rpc_url: &str, program_id: &str) -> Result<String> {
    let program = Pubkey::from_str(program_id).context("parse program id")?;
    let idl_address = anchor_idl_address(&program)?;
    let client = RpcClient::new(rpc_url.to_string());
    let account = client
        .get_account(&idl_address)
        .await
        .map_err(|err| anyhow::anyhow!(super::backfill::redact(err)))
        .with_context(|| format!("get_account {idl_address}"))?;

    let json = decode_idl_account_data(&account.data)?;
    stamp_address(&json, program_id)
}

/// Set the IDL's program address to `program_id`. Legacy Anchor IDLs carry
/// no `address`. The IDL account is derived from `program_id`, so an address
/// stored inside the IDL that names another program is not trusted: it would
/// let one program's IDL take over decoding for a different program.
fn stamp_address(json: &str, program_id: &str) -> Result<String> {
    let mut value: serde_json::Value = serde_json::from_str(json).context("parse fetched IDL")?;
    let Some(obj) = value.as_object_mut() else {
        return Ok(json.to_string());
    };
    let id = serde_json::Value::String(program_id.to_string());
    // Codama documents carry the id at `program.publicKey`.
    if obj.get("kind").and_then(serde_json::Value::as_str) == Some("rootNode") {
        let Some(program) = obj
            .get_mut("program")
            .and_then(serde_json::Value::as_object_mut)
        else {
            return Ok(json.to_string());
        };
        program.insert("publicKey".into(), id);
    } else {
        obj.insert("address".into(), id);
    }
    serde_json::to_string_pretty(&value).context("re-serialize stamped IDL")
}

/// Inflated IDL size limit. Real IDLs are below 1 MiB; the cap stops a zlib
/// bomb in the IDL account.
const MAX_IDL_BYTES: u64 = 16 * 1024 * 1024;

fn decode_idl_account_data(data: &[u8]) -> Result<String> {
    // Anchor IdlAccount: 8-byte disc + 32-byte authority + u32 data_len + zlib payload
    if data.len() < 44 {
        anyhow::bail!("IDL account data too short ({} bytes)", data.len());
    }
    let data_len = u32::from_le_bytes(data[40..44].try_into().context("data_len")?) as usize;
    let end = 44usize
        .checked_add(data_len)
        .filter(|e| *e <= data.len())
        .context("IDL data_len out of bounds")?;
    let compressed = &data[44..end];
    let mut json = Vec::new();
    ZlibDecoder::new(compressed)
        .take(MAX_IDL_BYTES + 1)
        .read_to_end(&mut json)
        .context("zlib decompress IDL")?;
    if json.len() as u64 > MAX_IDL_BYTES {
        anyhow::bail!("inflated IDL is larger than {MAX_IDL_BYTES} bytes");
    }
    String::from_utf8(json).context("IDL is not UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_missing_address() {
        let out = stamp_address(r#"{"name":"drift","version":"2.0.0"}"#, "dRifty111").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["address"], "dRifty111");
    }

    #[test]
    fn replaces_foreign_address() {
        let out = stamp_address(r#"{"address":"Other111","name":"demo"}"#, "dRifty111").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["address"], "dRifty111");
    }

    #[test]
    fn replaces_foreign_codama_public_key() {
        let src = r#"{"kind":"rootNode","standard":"codama","program":{"publicKey":"Other111"}}"#;
        let v: serde_json::Value =
            serde_json::from_str(&stamp_address(src, "dRifty111").unwrap()).unwrap();
        assert_eq!(v["program"]["publicKey"], "dRifty111");
    }

    #[test]
    fn rejects_short_idl_account() {
        assert!(decode_idl_account_data(&[0u8; 10]).is_err());
    }

    fn idl_account(inflated: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(inflated).unwrap();
        let payload = enc.finish().unwrap();
        let mut data = vec![0u8; 40];
        data.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
        data.extend_from_slice(&payload);
        data
    }

    #[test]
    fn inflates_idl_account() {
        assert_eq!(decode_idl_account_data(&idl_account(b"{}")).unwrap(), "{}");
    }

    #[test]
    fn rejects_zlib_bomb() {
        let bomb = vec![b' '; usize::try_from(MAX_IDL_BYTES).unwrap() + 1];
        let err = decode_idl_account_data(&idl_account(&bomb)).unwrap_err();
        assert!(err.to_string().contains("larger than"), "{err:#}");
    }
}

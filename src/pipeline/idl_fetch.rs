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
        .with_context(|| format!("get_account {idl_address}"))?;

    decode_idl_account_data(&account.data)
}

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
    let mut decoder = ZlibDecoder::new(compressed);
    let mut json = String::new();
    decoder
        .read_to_string(&mut json)
        .context("zlib decompress IDL")?;
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_short_idl_account() {
        assert!(decode_idl_account_data(&[0u8; 10]).is_err());
    }
}

use super::event::{IndexEvent, RawInstruction, TOKEN_2022_PROGRAM, TOKEN_PROGRAM};

/// SPL Token / Token-2022 instruction tags we care about.
const TRANSFER: u8 = 3;
const TRANSFER_CHECKED: u8 = 12;

pub fn parse_token_ix(ix: &RawInstruction) -> Option<IndexEvent> {
    if ix.program_id != TOKEN_PROGRAM && ix.program_id != TOKEN_2022_PROGRAM {
        return None;
    }
    let tag = *ix.data.first()?;
    match tag {
        TRANSFER => {
            if ix.data.len() < 9 || ix.accounts.len() < 2 {
                return None;
            }
            let amount = u64::from_le_bytes(ix.data[1..9].try_into().ok()?);
            Some(IndexEvent::TokenTransfer {
                slot: ix.slot,
                signature: ix.signature.clone(),
                ix_index: ix.ix_index,
                program: ix.program_id.clone(),
                source: ix.accounts[0].clone(),
                destination: ix.accounts[1].clone(),
                authority: ix.accounts.get(2).cloned(),
                mint: None,
                amount,
                decimals: None,
            })
        }
        TRANSFER_CHECKED => {
            if ix.data.len() < 10 || ix.accounts.len() < 3 {
                return None;
            }
            let amount = u64::from_le_bytes(ix.data[1..9].try_into().ok()?);
            let decimals = ix.data[9];
            Some(IndexEvent::TokenTransfer {
                slot: ix.slot,
                signature: ix.signature.clone(),
                ix_index: ix.ix_index,
                program: ix.program_id.clone(),
                source: ix.accounts[0].clone(),
                destination: ix.accounts[2].clone(),
                authority: ix.accounts.get(3).cloned(),
                mint: ix.accounts.get(1).cloned(),
                amount,
                decimals: Some(decimals),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ix(program: &str, accounts: &[&str], data: Vec<u8>) -> RawInstruction {
        RawInstruction {
            slot: 42,
            signature: "sig".into(),
            ix_index: 1,
            program_id: program.into(),
            accounts: accounts.iter().map(|s| (*s).to_string()).collect(),
            data,
        }
    }

    #[test]
    fn parses_spl_token_transfer() {
        let mut data = vec![TRANSFER];
        data.extend_from_slice(&1_000u64.to_le_bytes());
        let event = parse_token_ix(&ix(TOKEN_PROGRAM, &["src", "dst", "auth"], data)).unwrap();
        match event {
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
        let event = parse_token_ix(&ix(
            TOKEN_2022_PROGRAM,
            &["src", "mint", "dst", "auth"],
            data,
        ))
        .unwrap();
        match event {
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
    fn ignores_unknown_program() {
        let mut data = vec![TRANSFER];
        data.extend_from_slice(&1u64.to_le_bytes());
        assert!(parse_token_ix(&ix("11111111111111111111111111111111", &["a", "b"], data)).is_none());
    }
}

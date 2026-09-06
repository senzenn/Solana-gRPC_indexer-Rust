use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use idl_drift::model::{AccountItem, Idl, Instruction};
use idl_drift::{coverage, diff, Severity};

use super::decode_args;
use super::event::{IndexEvent, NamedAccount, RawInstruction};

/// Instruction router built from one or more program IDLs.
#[derive(Debug, Default, Clone)]
pub struct IdlRouter {
    programs: HashMap<String, LoadedProgram>,
}

#[derive(Debug, Clone)]
struct LoadedProgram {
    name: String,
    idl: Idl,
    /// discriminator bytes -> instruction name
    instructions: HashMap<Vec<u8>, String>,
}

impl IdlRouter {
    pub fn from_dir(dir: &Path) -> Result<Self> {
        let mut router = Self::default();
        if !dir.exists() {
            return Ok(router);
        }
        for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            router.load_file(&path)?;
        }
        Ok(router)
    }

    pub fn load_file(&mut self, path: &Path) -> Result<()> {
        let src = fs::read_to_string(path)
            .with_context(|| format!("read IDL {}", path.display()))?;
        let idl = Idl::from_json(&src)
            .with_context(|| format!("parse IDL {}", path.display()))?;
        self.load_idl(&idl);
        Ok(())
    }

    pub fn load_idl(&mut self, idl: &Idl) {
        let mut instructions = HashMap::new();
        for ix in &idl.instructions {
            if ix.discriminator.is_empty() {
                continue;
            }
            instructions.insert(ix.discriminator.clone(), ix.name.clone());
        }
        self.programs.insert(
            idl.address.clone(),
            LoadedProgram {
                name: idl.metadata.name.clone(),
                idl: idl.clone(),
                instructions,
            },
        );
    }

    pub fn program_ids(&self) -> Vec<String> {
        self.programs.keys().cloned().collect()
    }

    pub fn parse(&self, ix: &RawInstruction) -> Option<IndexEvent> {
        let program = self.programs.get(&ix.program_id)?;
        if ix.data.len() < 8 {
            return None;
        }
        let disc = ix.data[..8].to_vec();
        let instruction = program.instructions.get(&disc)?;
        let ix_def = program
            .idl
            .instructions
            .iter()
            .find(|def| def.name == *instruction)?;

        let args = decode_args::decode_args(&program.idl, instruction, &ix.data)
            .ok()
            .filter(|value| {
                value
                    .as_object()
                    .map(|map| !map.is_empty())
                    .unwrap_or(false)
            });

        let named_accounts = named_accounts_from_ix(ix_def, &ix.accounts);

        Some(IndexEvent::ProgramIx {
            slot: ix.slot,
            signature: ix.signature.clone(),
            ix_index: ix.ix_index,
            program_id: ix.program_id.clone(),
            program_name: program.name.clone(),
            instruction: instruction.clone(),
            accounts: ix.accounts.clone(),
            args,
            named_accounts,
        })
    }
}

fn flatten_account_items(
    items: &[AccountItem],
) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    for item in items {
        match item {
            AccountItem::Composite { accounts, .. } => {
                out.extend(flatten_account_items(accounts));
            }
            AccountItem::Single(account) => {
                out.push((account.name.as_str(), account.writable));
            }
        }
    }
    out
}

fn named_accounts_from_ix(
    ix_def: &Instruction,
    account_pubkeys: &[String],
) -> Option<Vec<NamedAccount>> {
    let flat = flatten_account_items(&ix_def.accounts);
    if flat.is_empty() {
        return None;
    }

    let named: Vec<NamedAccount> = flat
        .into_iter()
        .enumerate()
        .filter_map(|(index, (name, writable))| {
            account_pubkeys.get(index).map(|pubkey| NamedAccount {
                name: name.to_string(),
                pubkey: pubkey.clone(),
                writable,
            })
        })
        .collect();

    if named.is_empty() {
        None
    } else {
        Some(named)
    }
}

/// Inspect + optional baseline diff before writing a new IDL into the registry.
pub fn gate_idl(new_src: &str, baseline_src: Option<&str>, force: bool) -> Result<GateReport> {
    let new_idl = Idl::from_json(new_src).context("parse new IDL")?;
    let cov = coverage(&new_idl);
    if !cov.fully_mapped() && !force {
        anyhow::bail!(
            "IDL has {} reachable unmapped (Generic) fields; pass --force to keep it. First: {}",
            cov.unmapped.len(),
            cov.unmapped
                .first()
                .map(|u| u.path.as_str())
                .unwrap_or("?")
        );
    }

    let mut breaking = Vec::new();
    let mut dangerous = Vec::new();
    if let Some(old_src) = baseline_src {
        let old_idl = Idl::from_json(old_src).context("parse baseline IDL")?;
        let report = diff(&old_idl, &new_idl);
        for change in &report.changes {
            match change.severity {
                Severity::Breaking => breaking.push(change.message.clone()),
                Severity::Dangerous => dangerous.push(change.message.clone()),
                Severity::Additive | Severity::Cosmetic => {}
            }
        }
        if !breaking.is_empty() && !force {
            anyhow::bail!(
                "Breaking IDL drift ({}). Pass --force to overwrite the parser baseline. {}",
                breaking.len(),
                breaking.join("; ")
            );
        }
    }

    Ok(GateReport {
        program_id: new_idl.address,
        name: new_idl.metadata.name,
        instruction_count: new_idl.instructions.len(),
        breaking,
        dangerous,
        unmapped: cov.unmapped.len(),
    })
}

#[derive(Debug, Clone)]
pub struct GateReport {
    pub program_id: String,
    pub name: String,
    pub instruction_count: usize,
    pub breaking: Vec<String>,
    pub dangerous: Vec<String>,
    pub unmapped: usize,
}

pub fn install_idl(idl_dir: &Path, src: &str, force: bool) -> Result<(PathBuf, GateReport)> {
    fs::create_dir_all(idl_dir)?;
    let idl = Idl::from_json(src).context("parse IDL")?;
    let dest = idl_dir.join(format!("{}.json", sanitize_name(&idl.metadata.name)));
    let baseline = if dest.exists() {
        Some(fs::read_to_string(&dest)?)
    } else {
        None
    };
    let report = gate_idl(src, baseline.as_deref(), force)?;
    fs::write(&dest, src).with_context(|| format!("write {}", dest.display()))?;
    Ok((dest, report))
}

fn sanitize_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "program".into()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SWAP_OLD: &str = r#"{
      "address": "Drift1111111111111111111111111111111111111",
      "metadata": {"name": "demo", "version": "0.1.0", "spec": "0.1.0"},
      "instructions": [{
        "name": "swap",
        "discriminator": [1,2,3,4,5,6,7,8],
        "accounts": [{"name": "pool", "writable": true}],
        "args": [{"name": "amount", "type": "u64"}]
      }]
    }"#;

    const SWAP_PLUS_IX: &str = r#"{
      "address": "Drift1111111111111111111111111111111111111",
      "metadata": {"name": "demo", "version": "0.1.0", "spec": "0.1.0"},
      "instructions": [
        {
          "name": "swap",
          "discriminator": [1,2,3,4,5,6,7,8],
          "accounts": [{"name": "pool", "writable": true}],
          "args": [{"name": "amount", "type": "u64"}]
        },
        {
          "name": "init",
          "discriminator": [9,9,9,9,9,9,9,9],
          "accounts": [{"name": "pool", "writable": true}],
          "args": []
        }
      ]
    }"#;

    const SWAP_WIDEN: &str = r#"{
      "address": "Drift1111111111111111111111111111111111111",
      "metadata": {"name": "demo", "version": "0.1.0", "spec": "0.1.0"},
      "instructions": [{
        "name": "swap",
        "discriminator": [1,2,3,4,5,6,7,8],
        "accounts": [{"name": "pool", "writable": true}],
        "args": [{"name": "amount", "type": "u128"}]
      }]
    }"#;

    #[test]
    fn routes_instruction_by_discriminator() {
        let idl = Idl::from_json(SWAP_OLD).unwrap();
        let mut router = IdlRouter::default();
        router.load_idl(&idl);
        let mut data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        data.extend_from_slice(&99u64.to_le_bytes());
        let event = router
            .parse(&RawInstruction {
                slot: 9,
                signature: "s".into(),
                ix_index: 0,
                program_id: "Drift1111111111111111111111111111111111111".into(),
                accounts: vec!["pool_pubkey".into()],
                data,
            })
            .unwrap();
        match event {
            IndexEvent::ProgramIx {
                instruction,
                args,
                named_accounts,
                ..
            } => {
                assert_eq!(instruction, "swap");
                assert_eq!(args.unwrap()["amount"], serde_json::json!(99));
                let named = named_accounts.unwrap();
                assert_eq!(named.len(), 1);
                assert_eq!(named[0].name, "pool");
                assert!(named[0].writable);
                assert_eq!(named[0].pubkey, "pool_pubkey");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn additive_idl_is_allowed() {
        let report = gate_idl(SWAP_PLUS_IX, Some(SWAP_OLD), false).unwrap();
        assert!(report.breaking.is_empty());
        assert_eq!(report.instruction_count, 2);
    }

    #[test]
    fn breaking_idl_is_rejected_without_force() {
        let err = gate_idl(SWAP_WIDEN, Some(SWAP_OLD), false).unwrap_err();
        assert!(err.to_string().contains("Breaking"));
    }

    #[test]
    fn breaking_idl_is_allowed_with_force() {
        let report = gate_idl(SWAP_WIDEN, Some(SWAP_OLD), true).unwrap();
        assert!(!report.breaking.is_empty());
    }
}

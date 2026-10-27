use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use idl_drift::model::{AccountItem, Idl, Instruction};
use idl_drift::{Severity, coverage, diff};
use solana_sdk::pubkey::Pubkey;
use tracing::warn;

use super::event::{IndexEvent, NamedAccount, RawInstruction};
use super::{compiled, decode_args};

/// Instruction router built from one or more program IDLs.
#[derive(Debug, Default, Clone)]
pub struct IdlRouter {
    programs: HashMap<String, LoadedProgram>,
}

#[derive(Debug, Clone)]
struct LoadedProgram {
    name: String,
    idl: Idl,
    /// IDL types by name, for arg decoding.
    types: decode_args::TypeMap,
    /// discriminator bytes -> index in `idl.instructions`
    instructions: HashMap<Vec<u8>, usize>,
    /// Distinct discriminator lengths, longest first.
    disc_lens: Vec<usize>,
}

impl IdlRouter {
    /// Load every `*.json` IDL in `dir`. A file that does not load is logged
    /// and skipped, so one bad file does not stop startup.
    pub fn from_dir(dir: &Path) -> Result<Self> {
        let mut router = Self::default();
        if !dir.exists() {
            return Ok(router);
        }
        for path in idl_files(dir)? {
            if let Err(err) = router.load_file(&path) {
                warn!(path = %path.display(), error = %format!("{err:#}"), "skip IDL file");
            }
        }
        Ok(router)
    }

    pub fn load_file(&mut self, path: &Path) -> Result<()> {
        let src =
            fs::read_to_string(path).with_context(|| format!("read IDL {}", path.display()))?;
        let idl = Idl::from_json(&src).with_context(|| format!("parse IDL {}", path.display()))?;
        self.load_idl(&idl)
            .with_context(|| format!("load IDL {}", path.display()))
    }

    pub fn load_idl(&mut self, idl: &Idl) -> Result<()> {
        check_address(idl)?;
        check_name(idl)?;
        if self.programs.contains_key(&idl.address) {
            bail!("an IDL for program {} is already loaded", idl.address);
        }
        let mut instructions = HashMap::new();
        for (index, ix) in idl.instructions.iter().enumerate() {
            if ix.discriminator.is_empty() {
                continue;
            }
            instructions.insert(ix.discriminator.clone(), index);
        }
        let mut disc_lens: Vec<usize> = instructions.keys().map(Vec::len).collect();
        disc_lens.sort_unstable_by(|a, b| b.cmp(a));
        disc_lens.dedup();
        self.programs.insert(
            idl.address.clone(),
            LoadedProgram {
                name: idl.metadata.name.clone(),
                idl: idl.clone(),
                types: decode_args::type_map(idl),
                instructions,
                disc_lens,
            },
        );
        Ok(())
    }

    pub fn program_ids(&self) -> Vec<String> {
        self.programs.keys().cloned().collect()
    }

    pub fn parse(&self, ix: &RawInstruction) -> Option<IndexEvent> {
        // An IDL file wins over a compiled parser. idl_watch can then fix a
        // program upgrade without a new build.
        let Some(program) = self.programs.get(&ix.program_id) else {
            return compiled::parse(ix);
        };
        let index = program
            .disc_lens
            .iter()
            .find_map(|&len| program.instructions.get(ix.data.get(..len)?))?;
        let ix_def = program.idl.instructions.get(*index)?;

        let args = decode_args::decode_args(&program.types, ix_def, &ix.data);
        let args = Some(args).filter(|value| {
            value
                .as_object()
                .map(|map| !map.is_empty())
                .unwrap_or(false)
        });

        let named_accounts = named_accounts_from_ix(ix_def, &ix.accounts);

        Some(IndexEvent::ProgramIx {
            slot: ix.slot,
            signature: ix.signature.clone(),
            ix_index: ix.ix_index(),
            ix_path: ix.ix_path(),
            program_id: ix.program_id.clone(),
            program_name: program.name.clone(),
            instruction: ix_def.name.clone(),
            accounts: ix.accounts.clone(),
            args,
            named_accounts,
            failed: ix.failed,
        })
    }
}

/// The address keys the router and goes into the Geyser filter. Yellowstone
/// rejects the whole subscription when one entry is not a valid pubkey.
fn check_address(idl: &Idl) -> Result<()> {
    Pubkey::from_str(&idl.address).with_context(|| {
        format!(
            "IDL {:?} has no valid program address ({:?})",
            idl.metadata.name, idl.address
        )
    })?;
    Ok(())
}

/// Longest program name accepted. It is stored and shown as the `program` value.
const MAX_NAME_LEN: usize = 64;

fn check_name(idl: &Idl) -> Result<()> {
    let name = &idl.metadata.name;
    if name.len() > MAX_NAME_LEN {
        bail!(
            "IDL name has {} bytes; the limit is {MAX_NAME_LEN}",
            name.len()
        );
    }
    if name.chars().any(is_unprintable) {
        bail!("IDL name {name:?} has a non-printable character");
    }
    Ok(())
}

/// `*.json` files in `dir`, sorted so duplicate handling does not depend on
/// directory order.
fn idl_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

/// Find the IDL file in `idl_dir` that holds `program_id`. Files that do not
/// parse are skipped.
pub fn find_idl_file(idl_dir: &Path, program_id: &str) -> Result<Option<PathBuf>> {
    if !idl_dir.exists() {
        return Ok(None);
    }
    for path in idl_files(idl_dir)? {
        let parsed = fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|src| Idl::from_json(&src).map_err(anyhow::Error::from));
        match parsed {
            Ok(idl) if idl.address == program_id => return Ok(Some(path)),
            Ok(_) => {}
            Err(err) => warn!(path = %path.display(), error = %err, "skip IDL file"),
        }
    }
    Ok(None)
}

fn flatten_account_items(items: &[AccountItem]) -> Vec<(&str, bool)> {
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
                writable: Some(writable),
            })
        })
        .collect();

    if named.is_empty() { None } else { Some(named) }
}

/// Inspect + optional baseline diff before writing a new IDL into the registry.
pub fn gate_idl(new_src: &str, baseline_src: Option<&str>, force: bool) -> Result<GateReport> {
    let new_idl = Idl::from_json(new_src).context("parse new IDL")?;
    check_address(&new_idl)?;
    check_name(&new_idl)?;
    let cov = coverage(&new_idl);
    if !cov.fully_mapped() && !force {
        anyhow::bail!(
            "IDL has {} reachable unmapped (Generic) fields; pass --force to keep it. First: {}",
            cov.unmapped.len(),
            printable(cov.unmapped.first().map(|u| u.path.as_str()).unwrap_or("?"))
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
                printable(&breaking.join("; "))
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

/// Gate `src` and write it into `idl_dir`. The file is keyed by program
/// address: an existing file for that address is the drift baseline and is
/// replaced, otherwise the IDL goes to `<address>.json`.
pub fn install_idl(idl_dir: &Path, src: &str, force: bool) -> Result<(PathBuf, GateReport)> {
    fs::create_dir_all(idl_dir)?;
    let idl = Idl::from_json(src).context("parse IDL")?;
    check_address(&idl)?;
    let existing = find_idl_file(idl_dir, &idl.address)?;
    let baseline = match &existing {
        Some(path) => {
            Some(fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?)
        }
        None => None,
    };
    let dest = match existing {
        Some(path) => path,
        None => {
            let dest = idl_dir.join(format!("{}.json", idl.address));
            if dest.exists() {
                bail!(
                    "refuse to overwrite {}: it does not hold a valid IDL for {}",
                    dest.display(),
                    idl.address
                );
            }
            dest
        }
    };
    let report = gate_idl(src, baseline.as_deref(), force)?;
    write_atomic(&dest, src)?;
    Ok((dest, report))
}

/// IDL names and diff messages come from chain. Replace control characters
/// and bidi/zero-width marks so they cannot change what the terminal shows.
pub fn printable(s: &str) -> String {
    s.chars()
        .map(|c| if is_unprintable(c) { '?' } else { c })
        .collect()
}

fn is_unprintable(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Write to a temp file in the same directory, then rename it over `dest`.
/// A concurrent reader sees the old file or the new one, never a torn file.
fn write_atomic(dest: &Path, contents: &str) -> Result<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("bad IDL path {}", dest.display()))?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dest.with_file_name(format!(".{name}.{}.{nanos}.{seq}.tmp", std::process::id()));
    // create_new fails on an existing path, so a planted symlink is not followed.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .with_context(|| format!("create {}", tmp.display()))?;
    let written = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(err) = written.and_then(|()| fs::rename(&tmp, dest)) {
        let _ = fs::remove_file(&tmp);
        return Err(err).with_context(|| format!("write {}", dest.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SWAP_OLD: &str = r#"{
      "address": "dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH",
      "metadata": {"name": "demo", "version": "0.1.0", "spec": "0.1.0"},
      "instructions": [{
        "name": "swap",
        "discriminator": [1,2,3,4,5,6,7,8],
        "accounts": [{"name": "pool", "writable": true}],
        "args": [{"name": "amount", "type": "u64"}]
      }]
    }"#;

    const SWAP_PLUS_IX: &str = r#"{
      "address": "dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH",
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
      "address": "dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH",
      "metadata": {"name": "demo", "version": "0.1.0", "spec": "0.1.0"},
      "instructions": [{
        "name": "swap",
        "discriminator": [1,2,3,4,5,6,7,8],
        "accounts": [{"name": "pool", "writable": true}],
        "args": [{"name": "amount", "type": "u128"}]
      }]
    }"#;

    /// A pre-0.30 Anchor IDL as `parser fetch` pulls it off chain: no
    /// `address`, no instruction discriminators. Stamping the program id and
    /// deriving `sha256("global:<snake>")[..8]` must make it routable.
    const LEGACY_ANCHOR_FETCHED: &str = r#"{
      "version": "2.0.0",
      "name": "drift",
      "address": "dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH",
      "instructions": [{
        "name": "initializeUser",
        "accounts": [{"name": "user", "isMut": true, "isSigner": false}],
        "args": [{"name": "subAccountId", "type": "u16"}]
      }]
    }"#;

    #[test]
    fn routes_legacy_anchor_instruction_with_derived_discriminator() {
        let idl = Idl::from_json(LEGACY_ANCHOR_FETCHED).unwrap();
        let mut router = IdlRouter::default();
        router.load_idl(&idl).unwrap();
        assert_eq!(
            router.program_ids(),
            vec!["dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH".to_string()]
        );

        // Discriminator bytes taken from a real Drift v2 `initializeUser`
        // instruction on mainnet (tx 5iTzjpkp…hYTR, slot 447769258).
        let mut data = vec![111, 17, 185, 250, 60, 122, 38, 254];
        data.extend_from_slice(&7u16.to_le_bytes());
        let event = router
            .parse(&RawInstruction {
                slot: 447_769_258,
                signature: "5iTzjpkp".into(),
                path: crate::pipeline::ix_path::IxPath::root(3),
                program_id: "dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH".into(),
                accounts: vec!["user_pubkey".into()],
                data,
                failed: false,
            })
            .expect("legacy Anchor instruction should route");
        match event {
            IndexEvent::ProgramIx {
                program_name,
                instruction,
                args,
                ..
            } => {
                assert_eq!(program_name, "drift");
                assert_eq!(instruction, "initializeUser");
                assert_eq!(args.unwrap()["subAccountId"], serde_json::json!(7));
            }
            other => panic!("expected ProgramIx, got {other:?}"),
        }
    }

    #[test]
    fn routes_instruction_by_discriminator() {
        let idl = Idl::from_json(SWAP_OLD).unwrap();
        let mut router = IdlRouter::default();
        router.load_idl(&idl).unwrap();
        let mut data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        data.extend_from_slice(&99u64.to_le_bytes());
        let event = router
            .parse(&RawInstruction {
                slot: 9,
                signature: "s".into(),
                path: crate::pipeline::ix_path::IxPath::root(0),
                program_id: "dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH".into(),
                accounts: vec!["pool_pubkey".into()],
                data,
                failed: false,
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
                assert_eq!(named[0].writable, Some(true));
                assert_eq!(named[0].pubkey, "pool_pubkey");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn failed_tx_marker_reaches_payload() {
        let idl = Idl::from_json(SWAP_OLD).unwrap();
        let mut router = IdlRouter::default();
        router.load_idl(&idl).unwrap();
        let mut data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        data.extend_from_slice(&99u64.to_le_bytes());
        let ix = RawInstruction {
            slot: 9,
            signature: "s".into(),
            path: crate::pipeline::ix_path::IxPath::root(0),
            program_id: "dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH".into(),
            accounts: vec!["pool_pubkey".into()],
            data,
            failed: false,
        };
        let payload = |failed: bool| {
            let ixs = crate::pipeline::event::failed_tx_policy(vec![ix.clone()], failed, true);
            serde_json::to_value(router.parse(&ixs[0]).unwrap()).unwrap()
        };
        // Payloads of successful transactions stay the same, so stored rows do not change.
        assert!(payload(false).get("failed").is_none());
        assert_eq!(payload(true)["failed"], serde_json::json!(true));
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

    /// Codama / native programs dispatch on a 1-byte tag, and an instruction
    /// can be shorter than 8 bytes.
    #[test]
    fn routes_one_byte_discriminator() {
        let idl = Idl::from_json(
            r#"{
              "address": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
              "metadata": {"name": "tag", "version": "0.1.0", "spec": "0.1.0"},
              "instructions": [{
                "name": "approve",
                "discriminator": [4],
                "accounts": [{"name": "source", "writable": true}],
                "args": [{"name": "level", "type": "u8"}]
              }]
            }"#,
        )
        .unwrap();
        let mut router = IdlRouter::default();
        router.load_idl(&idl).unwrap();
        let event = router
            .parse(&RawInstruction {
                slot: 1,
                signature: "s".into(),
                path: crate::pipeline::ix_path::IxPath::root(0),
                program_id: "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".into(),
                accounts: vec!["src".into()],
                data: vec![4, 7],
                failed: false,
            })
            .expect("1-byte discriminator should route");
        match event {
            IndexEvent::ProgramIx {
                instruction, args, ..
            } => {
                assert_eq!(instruction, "approve");
                assert_eq!(args.unwrap()["level"], serde_json::json!(7));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_idl_without_program_address() {
        let legacy = r#"{"version": "0.1.0", "name": "nameless", "instructions": []}"#;
        assert!(gate_idl(legacy, None, true).is_err());
        let idl = Idl::from_json(legacy).unwrap();
        assert!(IdlRouter::default().load_idl(&idl).is_err());
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("idl-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn install_keys_by_address_so_rename_still_gates() {
        let dir = scratch_dir("install");
        let (dest, _) = install_idl(&dir, SWAP_OLD, false).unwrap();
        assert_eq!(
            dest,
            dir.join("dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH.json")
        );

        let renamed = SWAP_WIDEN.replace(r#""name": "demo""#, r#""name": "demo_v2""#);
        let err = install_idl(&dir, &renamed, false).unwrap_err();
        assert!(err.to_string().contains("Breaking"), "{err:#}");

        let (forced, _) = install_idl(&dir, &renamed, true).unwrap();
        assert_eq!(forced, dest);
        assert_eq!(
            files_in(&dir),
            vec!["dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH.json"]
        );
        assert_eq!(fs::read_to_string(&dest).unwrap(), renamed);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn from_dir_skips_torn_and_duplicate_files() {
        let dir = scratch_dir("from-dir");
        fs::write(dir.join("a_torn.json"), "{\"address\": ").unwrap();
        fs::write(dir.join("b_drift.json"), SWAP_OLD).unwrap();
        fs::write(dir.join("c_drift_copy.json"), SWAP_WIDEN).unwrap();
        let router = IdlRouter::from_dir(&dir).unwrap();
        assert_eq!(
            router.program_ids(),
            vec!["dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH".to_string()]
        );
        let program = &router.programs["dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH"];
        assert_eq!(
            program.idl.instructions[0].args[0].ty,
            idl_drift::model::Type::U64
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn printable_strips_terminal_escapes() {
        assert_eq!(printable("a\u{1b}]0;x\u{7}\u{9b}2Jb"), "a?]0;x??2Jb");
        assert_eq!(
            printable("a\u{200B}b\u{202E}c\u{2066}d\u{2069}e\u{200F}"),
            "a?b?c?d?e?"
        );
    }

    #[test]
    fn rejects_long_or_unprintable_program_name() {
        let with_name = |name: &str| {
            let json = serde_json::to_string(name).unwrap();
            SWAP_OLD.replace(r#""name": "demo""#, &format!(r#""name": {json}"#))
        };
        for name in ["x".repeat(65), "evil\u{1b}[2J".into(), "rtl\u{202E}".into()] {
            let src = with_name(&name);
            assert!(gate_idl(&src, None, true).is_err(), "{name:?}");
            let idl = Idl::from_json(&src).unwrap();
            assert!(IdlRouter::default().load_idl(&idl).is_err(), "{name:?}");
        }
        assert!(gate_idl(&with_name(&"x".repeat(64)), None, true).is_ok());
    }

    #[test]
    fn shipped_idls_load() {
        let router = IdlRouter::from_dir(Path::new("idls")).unwrap();
        let ids = router.program_ids();
        assert!(
            ids.contains(&"6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P".to_string()),
            "{ids:?}"
        );
    }

    #[test]
    fn write_atomic_removes_temp_file_on_error() {
        let dir = scratch_dir("atomic");
        let dest = dir.join("x.json");
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("keep"), "1").unwrap();
        assert!(write_atomic(&dest, "{}").is_err());
        assert_eq!(files_in(&dir), vec!["x.json"]);
        fs::remove_dir_all(&dir).unwrap();
    }
}

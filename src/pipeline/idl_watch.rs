use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use idl_drift::{diff, Severity};
use tokio::time::sleep;
use tracing::{info, warn};

use super::idl::{install_idl, IdlRouter};
use super::idl_fetch::fetch_idl_json;

pub struct WatchConfig {
    pub rpc_url: String,
    pub idl_dir: std::path::PathBuf,
    pub interval_secs: u64,
    pub force_on_breaking: bool,
}

pub async fn watch(cfg: WatchConfig) -> Result<()> {
    let router = IdlRouter::from_dir(&cfg.idl_dir)?;
    let program_ids = router.program_ids();
    if program_ids.is_empty() {
        anyhow::bail!("no IDLs in {}", cfg.idl_dir.display());
    }

    info!(
        programs = program_ids.len(),
        interval_secs = cfg.interval_secs,
        "starting IDL watch"
    );

    loop {
        for program_id in &program_ids {
            if let Err(err) = check_one(&cfg, program_id).await {
                warn!(program_id, error = %err, "idl watch check failed");
            }
        }
        sleep(Duration::from_secs(cfg.interval_secs)).await;
    }
}

async fn check_one(cfg: &WatchConfig, program_id: &str) -> Result<()> {
    let fresh = fetch_idl_json(&cfg.rpc_url, program_id).await?;
    let baseline_path = find_idl_file(&cfg.idl_dir, program_id)?;
    let baseline = std::fs::read_to_string(&baseline_path)?;
    let old = idl_drift::model::Idl::from_json(&baseline)?;
    let new = idl_drift::model::Idl::from_json(&fresh)?;
    let report = diff(&old, &new);

    let mut breaking = 0usize;
    let mut dangerous = 0usize;
    for change in &report.changes {
        match change.severity {
            Severity::Breaking => breaking += 1,
            Severity::Dangerous => dangerous += 1,
            Severity::Additive | Severity::Cosmetic => {}
        }
    }

    if breaking == 0 && dangerous == 0 {
        return Ok(());
    }

    warn!(
        program_id,
        breaking,
        dangerous,
        "on-chain IDL drift detected"
    );

    if breaking > 0 && !cfg.force_on_breaking {
        warn!(program_id, "breaking drift — not updating (pass --force to overwrite)");
        return Ok(());
    }

    let (_dest, gate) = install_idl(&cfg.idl_dir, &fresh, cfg.force_on_breaking)?;
    info!(
        program_id,
        name = gate.name,
        breaking = gate.breaking.len(),
        dangerous = gate.dangerous.len(),
        "updated local IDL from chain"
    );
    Ok(())
}

fn find_idl_file(idl_dir: &Path, program_id: &str) -> Result<std::path::PathBuf> {
    for entry in std::fs::read_dir(idl_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let src = std::fs::read_to_string(&path)?;
        let idl = idl_drift::model::Idl::from_json(&src)?;
        if idl.address == program_id {
            return Ok(path);
        }
    }
    anyhow::bail!("no IDL file for program {program_id} in {}", idl_dir.display())
}

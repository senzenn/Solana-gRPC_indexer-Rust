use std::time::Duration;

use anyhow::Result;
use idl_drift::{Severity, diff};
use tokio::time::sleep;
use tracing::{info, warn};

use super::idl::{IdlRouter, find_idl_file, install_idl};
use super::idl_fetch::fetch_idl_json;

pub struct WatchConfig {
    pub rpc_url: String,
    pub idl_dir: std::path::PathBuf,
    pub interval_secs: u64,
    pub force_on_breaking: bool,
}

/// Shortest poll interval. Each poll sends one RPC call per program.
const MIN_INTERVAL_SECS: u64 = 10;

fn poll_interval(secs: u64) -> Duration {
    Duration::from_secs(secs.max(MIN_INTERVAL_SECS))
}

pub async fn watch(cfg: WatchConfig) -> Result<()> {
    let router = IdlRouter::from_dir(&cfg.idl_dir)?;
    let program_ids = router.program_ids();
    if program_ids.is_empty() {
        anyhow::bail!("no IDLs in {}", cfg.idl_dir.display());
    }

    let interval = poll_interval(cfg.interval_secs);
    info!(
        programs = program_ids.len(),
        interval_secs = interval.as_secs(),
        "starting IDL watch"
    );

    loop {
        for program_id in &program_ids {
            if let Err(err) = check_one(&cfg, program_id).await {
                warn!(program_id, error = %err, "idl watch check failed");
            }
        }
        sleep(interval).await;
    }
}

async fn check_one(cfg: &WatchConfig, program_id: &str) -> Result<()> {
    let fresh = fetch_idl_json(&cfg.rpc_url, program_id).await?;
    let baseline_path = find_idl_file(&cfg.idl_dir, program_id)?.ok_or_else(|| {
        anyhow::anyhow!(
            "no IDL file for program {program_id} in {}",
            cfg.idl_dir.display()
        )
    })?;
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
        breaking, dangerous, "on-chain IDL drift detected"
    );

    if breaking > 0 && !cfg.force_on_breaking {
        warn!(
            program_id,
            "breaking drift — not updating (pass --force to overwrite)"
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_interval_has_a_floor() {
        assert_eq!(poll_interval(0), Duration::from_secs(10));
        assert_eq!(poll_interval(300), Duration::from_secs(300));
    }
}

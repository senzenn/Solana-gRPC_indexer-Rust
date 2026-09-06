use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::idl::{install_idl, IdlRouter};
use super::idl_fetch::fetch_idl_json;

pub fn add_idl(idl_dir: &Path, idl_path: &Path, force: bool) -> Result<()> {
    let src = std::fs::read_to_string(idl_path)
        .with_context(|| format!("read {}", idl_path.display()))?;
    let (dest, report) = install_idl(idl_dir, &src, force)?;
    println!(
        "installed {} ({}) -> {}",
        report.name,
        report.program_id,
        dest.display()
    );
    println!("instructions: {}", report.instruction_count);
    if !report.dangerous.is_empty() {
        println!("dangerous: {}", report.dangerous.join("; "));
    }
    if !report.breaking.is_empty() {
        println!("breaking (forced): {}", report.breaking.join("; "));
    }
    if report.unmapped > 0 {
        println!("unmapped Generic fields: {}", report.unmapped);
    }
    Ok(())
}

pub fn list_idls(idl_dir: &Path) -> Result<()> {
    let router = IdlRouter::from_dir(idl_dir)?;
    let ids = router.program_ids();
    if ids.is_empty() {
        println!("no IDLs in {}", idl_dir.display());
        return Ok(());
    }
    for id in ids {
        println!("{id}");
    }
    Ok(())
}

pub fn diff_idls(old: &PathBuf, new: &PathBuf) -> Result<()> {
    let old_src = std::fs::read_to_string(old)?;
    let new_src = std::fs::read_to_string(new)?;
    let old_idl = idl_drift::model::Idl::from_json(&old_src)?;
    let new_idl = idl_drift::model::Idl::from_json(&new_src)?;
    let report = idl_drift::diff(&old_idl, &new_idl);
    let (b, d, a, c) = report.counts();
    println!("breaking={b} dangerous={d} additive={a} cosmetic={c}");
    for change in &report.changes {
        println!("{:?}  {}", change.severity, change.message);
    }
    if report.has_breaking() {
        anyhow::bail!("breaking IDL drift");
    }
    Ok(())
}

pub async fn fetch_idl(
    rpc_url: &str,
    program_id: &str,
    idl_dir: &Path,
    force: bool,
) -> Result<()> {
    let json = fetch_idl_json(rpc_url, program_id).await?;
    let (dest, report) = install_idl(idl_dir, &json, force)?;
    println!(
        "fetched {} ({}) -> {}",
        report.name, report.program_id, dest.display()
    );
    println!("instructions: {}", report.instruction_count);
    Ok(())
}

pub async fn watch_idls(
    rpc_url: &str,
    idl_dir: &Path,
    interval_secs: u64,
    force: bool,
) -> Result<()> {
    super::idl_watch::watch(super::idl_watch::WatchConfig {
        rpc_url: rpc_url.to_string(),
        idl_dir: idl_dir.to_path_buf(),
        interval_secs,
        force_on_breaking: force,
    })
    .await
}

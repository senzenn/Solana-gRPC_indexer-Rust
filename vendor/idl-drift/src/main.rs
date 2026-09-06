//! idl-drift CLI — `diff` and `inspect` subcommands.
//!
//! Exit codes for `diff` (oasdiff CI contract):
//!   0 = no Breaking changes
//!   1 = at least one Breaking change
//!   2 = I/O or parse error
//!
//! Exit codes for `inspect`:
//!   0 = zero reachable Generic (unmapped) fields
//!   1 = at least one reachable Generic field
//!   2 = I/O or parse error

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use idl_drift::model::Idl;
use idl_drift::{coverage, diff, Report, Severity};
use owo_colors::OwoColorize;
use serde::Serialize;

#[derive(Parser)]
#[command(
    name = "idl-drift",
    version,
    about = "Semantic drift detection for Solana program IDLs"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Diff two IDL JSON files and classify every change.
    Diff {
        /// Path to the baseline (old) IDL JSON.
        old: PathBuf,
        /// Path to the current (new) IDL JSON.
        new: PathBuf,
        /// Emit machine-readable JSON instead of the human report.
        #[arg(long)]
        json: bool,
    },
    /// Parse one IDL and report Codama→flat type coverage gaps (`Type::Generic`).
    Inspect {
        /// Path to an IDL JSON (Codama `RootNode` or Anchor/Solana-spec).
        idl: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Diff { old, new, json } => run_diff(&old, &new, json),
        Cmd::Inspect { idl } => run_inspect(&idl),
    }
}

fn run_inspect(path: &PathBuf) -> ExitCode {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return fail(format!("read {}: {e}", path.display())),
    };
    let idl = match Idl::from_json(&src) {
        Ok(v) => v,
        Err(e) => return fail(format!("parse {}: {e}", path.display())),
    };

    let report = coverage(&idl);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if let Err(e) = writeln!(
        out,
        "instructions: {}  accounts: {}  events: {}  types: {}",
        report.instructions, report.accounts, report.events, report.types
    ) {
        return fail(format!("write report: {e}"));
    }
    if let Err(e) = writeln!(out, "unmapped (Generic) fields: {}", report.unmapped.len()) {
        return fail(format!("write report: {e}"));
    }
    for u in &report.unmapped {
        if let Err(e) = writeln!(out, "UNMAPPED: {} -> Generic(\"{}\")", u.path, u.kind) {
            return fail(format!("write report: {e}"));
        }
    }

    if report.fully_mapped() {
        ExitCode::from(0)
    } else {
        ExitCode::from(1)
    }
}

fn run_diff(old_path: &PathBuf, new_path: &PathBuf, as_json: bool) -> ExitCode {
    let old_src = match std::fs::read_to_string(old_path) {
        Ok(s) => s,
        Err(e) => return fail(format!("read {}: {e}", old_path.display())),
    };
    let new_src = match std::fs::read_to_string(new_path) {
        Ok(s) => s,
        Err(e) => return fail(format!("read {}: {e}", new_path.display())),
    };

    let old = match Idl::from_json(&old_src) {
        Ok(v) => v,
        Err(e) => return fail(format!("parse {}: {e}", old_path.display())),
    };
    let new = match Idl::from_json(&new_src) {
        Ok(v) => v,
        Err(e) => return fail(format!("parse {}: {e}", new_path.display())),
    };

    let report = diff(&old, &new);
    let code = report.exit_code();

    if as_json {
        if let Err(e) = print_json(&report) {
            return fail(format!("write json: {e}"));
        }
    } else if let Err(e) = print_human(&report) {
        return fail(format!("write report: {e}"));
    }

    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

fn fail(msg: impl AsRef<str>) -> ExitCode {
    let _ = writeln!(io::stderr(), "error: {}", msg.as_ref());
    ExitCode::from(2)
}

fn print_human(report: &Report) -> io::Result<()> {
    let color = io::stdout().is_terminal();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    let groups: [(Severity, &str); 4] = [
        (Severity::Breaking, "BREAKING"),
        (Severity::Dangerous, "DANGEROUS"),
        (Severity::Additive, "ADDITIVE"),
        (Severity::Cosmetic, "COSMETIC"),
    ];

    for (sev, tag) in groups {
        let msgs: Vec<&str> = report
            .changes
            .iter()
            .filter(|c| c.severity == sev)
            .map(|c| c.message.as_str())
            .collect();
        if msgs.is_empty() {
            continue;
        }
        for msg in msgs {
            writeln!(out, "  [{}] {}", paint_tag(tag, sev, color), msg)?;
        }
    }

    let (b, d, a, c) = report.counts();
    writeln!(
        out,
        "summary: {b} breaking, {d} dangerous, {a} additive, {c} cosmetic"
    )?;
    writeln!(out, "exit code: {}", report.exit_code())?;
    Ok(())
}

fn paint_tag(tag: &str, sev: Severity, color: bool) -> String {
    if !color {
        return tag.to_string();
    }
    match sev {
        Severity::Breaking => tag.red().bold().to_string(),
        Severity::Dangerous => tag.yellow().bold().to_string(),
        Severity::Additive => tag.green().to_string(),
        Severity::Cosmetic => tag.dimmed().to_string(),
    }
}

#[derive(Serialize)]
struct JsonReport<'a> {
    breaking: Vec<&'a str>,
    dangerous: Vec<&'a str>,
    additive: Vec<&'a str>,
    cosmetic: Vec<&'a str>,
    counts: JsonCounts,
    exit_code: i32,
}

#[derive(Serialize)]
struct JsonCounts {
    breaking: usize,
    dangerous: usize,
    additive: usize,
    cosmetic: usize,
}

fn print_json(report: &Report) -> Result<(), serde_json::Error> {
    let (b, d, a, c) = report.counts();
    let view = JsonReport {
        breaking: msgs(report, Severity::Breaking),
        dangerous: msgs(report, Severity::Dangerous),
        additive: msgs(report, Severity::Additive),
        cosmetic: msgs(report, Severity::Cosmetic),
        counts: JsonCounts {
            breaking: b,
            dangerous: d,
            additive: a,
            cosmetic: c,
        },
        exit_code: report.exit_code(),
    };
    println!("{}", serde_json::to_string_pretty(&view)?);
    Ok(())
}

fn msgs(report: &Report, sev: Severity) -> Vec<&str> {
    report
        .changes
        .iter()
        .filter(|c| c.severity == sev)
        .map(|c| c.message.as_str())
        .collect()
}

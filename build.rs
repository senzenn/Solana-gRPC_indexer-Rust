use std::error::Error;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() -> Result<(), Box<dyn Error>> {
    tonic_prost_build::configure()
        .build_server(true)
        .compile_protos(&["src/proto/index_export.proto"], &["src/proto"])?;
    // sqlx::migrate! embeds the migrations. One rerun line turns off the default
    // rule, and tonic-prost-build prints none, so list the protos as well.
    println!("cargo:rerun-if-changed=src/proto");
    println!("cargo:rerun-if-changed=migrations");
    println!("cargo:rerun-if-changed=parsers");
    write_compiled_parsers()
}

/// Writes one `include_shipstern_parser!` call and one registry entry for each
/// IDL in `parsers/`. src/pipeline/compiled.rs includes the file.
fn write_compiled_parsers() -> Result<(), Box<dyn Error>> {
    let dir = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("parsers");
    let mut code = String::new();
    let mut entries = String::new();
    for path in idl_files(&dir)? {
        let shown = path.display();
        let text = path
            .to_str()
            .ok_or_else(|| format!("{shown}: path is not UTF-8"))?;
        let idl: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path)?)
            .map_err(|err| format!("{shown}: {err}"))?;
        let name = program_name(&idl).ok_or_else(|| format!("{shown}: no program name"))?;
        let module = to_snake_case(name);
        writeln!(
            code,
            "shipstern_proc_macro::include_shipstern_parser!({text:?});"
        )?;
        writeln!(
            entries,
            "    Entry {{ name: {name:?}, program_id: {module}::PROGRAM_ID, \
             parse: |update| run(&{module}::InstructionParser, update) }},"
        )?;
    }
    writeln!(
        code,
        "pub(super) static ENTRIES: &[Entry] = &[\n{entries}];"
    )?;
    fs::write(
        Path::new(&env::var("OUT_DIR")?).join("compiled_parsers.rs"),
        code,
    )?;
    Ok(())
}

fn idl_files(dir: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut paths = Vec::new();
    if !dir.exists() {
        return Ok(paths);
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

/// Codama keeps the name at `program.name`, Anchor 0.30+ at `metadata.name`,
/// and older Anchor at the top level.
fn program_name(idl: &serde_json::Value) -> Option<&str> {
    idl.pointer("/program/name")
        .or_else(|| idl.pointer("/metadata/name"))
        .or_else(|| idl.get("name"))
        .and_then(serde_json::Value::as_str)
}

/// The macro names its module with this rule (shipstern-proc-macro
/// `utils::to_snake_case`). A different result fails the build, not the run.
fn to_snake_case(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() && i > 0 {
            let prev_upper = chars[i - 1].is_uppercase();
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if !prev_upper || next_lower {
                out.push('_');
            }
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

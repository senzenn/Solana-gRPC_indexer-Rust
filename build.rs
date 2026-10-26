fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::configure()
        .build_server(true)
        .compile_protos(&["src/proto/index_export.proto"], &["src/proto"])?;
    // sqlx::migrate! embeds the migrations. One rerun line turns off the default
    // rule, and tonic-prost-build prints none, so list the protos as well.
    println!("cargo:rerun-if-changed=src/proto");
    println!("cargo:rerun-if-changed=migrations");
    Ok(())
}

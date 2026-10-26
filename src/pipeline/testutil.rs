/// A SQLite URL in its own temp dir. Drop removes the dir with the -wal and
/// -shm files, also when the test panics.
pub struct TempDb {
    _dir: tempfile::TempDir,
    pub url: String,
}

impl TempDb {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let url = format!("sqlite:{}", dir.path().join("index.db").display());
        Self { _dir: dir, url }
    }
}

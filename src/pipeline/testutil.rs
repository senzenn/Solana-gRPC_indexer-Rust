use std::sync::Arc;

use super::event::{IndexEvent, RawInstruction, TOKEN_PROGRAM};
use super::ix_path::IxPath;
use super::store::EventStore;

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

/// Keep the TempDb alive while the store is in use.
pub async fn temp_store() -> (Arc<EventStore>, TempDb) {
    let db = TempDb::new();
    let store = EventStore::open(&db.url).await.expect("open store");
    (Arc::new(store), db)
}

pub fn transfer(slot: u64, signature: &str) -> IndexEvent {
    IndexEvent::TokenTransfer {
        slot,
        signature: signature.into(),
        ix_index: 0,
        ix_path: "0".into(),
        program: TOKEN_PROGRAM.into(),
        source: "a".into(),
        destination: "b".into(),
        authority: None,
        mint: None,
        amount: 1,
        decimals: None,
    }
}

/// An SPL Token `Transfer` of 1000 at path 0.
pub fn spl_transfer_ix(slot: u64) -> RawInstruction {
    let mut data = vec![3u8];
    data.extend_from_slice(&1_000u64.to_le_bytes());
    RawInstruction {
        slot,
        signature: format!("sig-{slot}"),
        path: IxPath::root(0),
        program_id: TOKEN_PROGRAM.into(),
        accounts: vec!["src".into(), "dst".into(), "auth".into()],
        data,
        failed: false,
    }
}

//! Writes the version-1 saved-world fixture through the store's own operations: a created
//! world, then two checkpoints, each in a writing session that closes in order. The second
//! checkpoint supersedes a chunk revision of the first, which reclamation removes. The content
//! (cells with boundary words, a body set with free, live and retired slots and every body
//! field) is `tests/fixtures/v1/content.rs`.
//!
//! ```text
//! cargo run --locked -p world-store --example capture_v1_fixture -- OUT.redb
//! ```
//!
//! It captured `tests/fixtures/v1/world-v1.redb` once (see that directory's README). The
//! fixture is immutable: a capture by changed code is a new fixture set, never an update.

use std::{path::PathBuf, process::ExitCode};

use world_runtime::{
    ids::SessionEpoch,
    store::{CheckpointId, ChunkRow, Publication, StoreError, WorldStore},
};
use world_store::{faults::StoreFaults, store::RedbWorldStore};

#[path = "../tests/fixtures/v1/content.rs"]
mod content;

fn capture(out: &std::path::Path) -> Result<(), StoreError> {
    let store = RedbWorldStore::create(
        out,
        &content::header(),
        &Publication {
            id: CheckpointId::FIRST,
            tick: None,
            config: content::config(1),
            window: content::window(),
            epoch: SessionEpoch::FIRST,
            heads: &[],
            bodies: 0,
        },
        StoreFaults::new(),
    )?;
    for content::Checkpoint {
        id,
        tick,
        epoch,
        revisions,
    } in content::checkpoints()
    {
        store.begin_session()?;
        let payloads: Vec<_> = revisions
            .iter()
            .map(|(key, revision)| (*key, *revision, content::payload(*key, *revision)))
            .collect();
        let rows: Vec<ChunkRow<'_>> = payloads
            .iter()
            .map(|(key, revision, payload)| ChunkRow {
                key: *key,
                revision: content::revision(*revision),
                payload,
            })
            .collect();
        store.write_chunks(&rows)?;
        let bodies = content::bodies(id);
        store.write_bodies(CheckpointId::new(id), &bodies)?;
        let heads: Vec<_> = revisions
            .iter()
            .map(|(key, revision)| (*key, content::revision(*revision)))
            .collect();
        store.publish(&Publication {
            id: CheckpointId::new(id),
            tick: Some(tick),
            config: content::config(id),
            window: content::window(),
            epoch,
            heads: &heads,
            bodies: bodies.live() as u32,
        })?;
        store.end_session()?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let Some(out) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: capture_v1_fixture OUT.redb");
        return ExitCode::from(2);
    };
    match capture(&out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("capture_v1_fixture: {error}");
            ExitCode::FAILURE
        }
    }
}

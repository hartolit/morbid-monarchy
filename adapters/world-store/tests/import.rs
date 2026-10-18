//! Importing legacy stores written by the old writer (`fixtures/legacy-v0`): a complete new world
//! from them, explicit refusals of rows it cannot hold, and originals that stay byte for byte as
//! they were whatever happens to the import.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use world_model::{
    command::{AmbientTemperature, ElevationScale},
    rules::SimulationRules,
    spatial::ChunkKey,
};
use world_runtime::{
    ids::{ChunkRevision, WorldId},
    port::ResidentWindow,
    store::{ChunkPayload, PhysicalConfig, StoreError, WorldOrigin, WorldStore},
};
use world_store::{
    faults::{Action, Barrier, StoreFaults},
    legacy::{self, ImportError, ImportOptions, LegacyStore, NonPlanarRows},
    store::{OpenOptionsStore, RedbWorldStore},
};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "world-store-import-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        fs::create_dir_all(&dir).expect("scratch directory");
        Self(dir)
    }

    fn path(&self, file: &str) -> PathBuf {
        self.0.join(file)
    }

    /// A copy of the old writer's store, which the test may treat as a user's original.
    fn legacy_copy(&self) -> PathBuf {
        let copy = self.path("save.redb");
        fs::copy(fixtures().join("legacy-chunks.redb"), &copy).expect("copy fixture");
        copy
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy-v0")
}

fn options(non_planar: NonPlanarRows) -> ImportOptions {
    ImportOptions {
        world: WorldId::new(0x001E_6AC7),
        seed: 69,
        rule_seed: 69,
        config: PhysicalConfig {
            rules: SimulationRules::ALL,
            ambient_temperature: AmbientTemperature::celsius(20.0).expect("supported"),
            elevation_scale: ElevationScale::new(0.5).expect("supported"),
        },
        window: ResidentWindow::new(ChunkKey::new(0, 0, 0), 6, 6).expect("window"),
        non_planar,
        rows_per_transaction: 2,
    }
}

const PLANAR: [ChunkKey; 4] = [
    ChunkKey {
        key: world_model::glam::IVec3::new(0, 0, 0),
    },
    ChunkKey {
        key: world_model::glam::IVec3::new(-1, -2, 0),
    },
    ChunkKey {
        key: world_model::glam::IVec3::new(2, 1, 0),
    },
    ChunkKey {
        key: world_model::glam::IVec3::new(7, -3, 0),
    },
];

fn row_bytes(key: ChunkKey) -> Vec<u8> {
    let name = format!("{}_{}_{}.bitcode", key.key.x, key.key.y, key.key.z);
    fs::read(fixtures().join("rows").join(name)).expect("fixture row")
}

#[test]
fn the_old_writer_s_rows_become_a_complete_world_and_the_original_is_unchanged() {
    let scratch = Scratch::new("complete");
    let original = scratch.legacy_copy();
    let before = fs::read(&original).expect("bytes");
    let destination = scratch.path("world.redb");
    let report = legacy::import(
        &original,
        &destination,
        &options(NonPlanarRows::Skip),
        StoreFaults::new(),
    )
    .expect("imports");
    assert_eq!(
        fs::read(&original).expect("bytes"),
        before,
        "the original was not touched"
    );
    assert_eq!(report.interpretation.seed, 69);
    assert_eq!(report.interpretation.elevation_scale.value(), 0.5);
    assert_eq!(report.interpretation.rows_imported, 4);
    assert_eq!(report.interpretation.rows_refused, 1);
    assert_eq!(report.refused_keys, vec![ChunkKey::new(0, 0, 1)]);
    assert_eq!(report.last_simulated, Some((0.0, 1234.5)));
    assert_eq!(report.read_repaired_copy, None);

    let store = RedbWorldStore::open(&destination, OpenOptionsStore::default()).expect("opens");
    assert_eq!(
        store.header().origin,
        WorldOrigin::LegacyImport(report.interpretation)
    );
    assert_eq!(store.header().generator.seed, 69);
    let latest = store.latest().expect("published");
    assert_eq!((latest.chunks, latest.bodies, latest.tick), (4, 0, None));
    assert_eq!(latest.config.elevation_scale.value(), 0.5);
    assert_eq!(store.verify().expect("verifies").chunks, 4);
    let mut payload = ChunkPayload::zeroed();
    for key in PLANAR {
        let row = legacy::decode(&row_bytes(key)).expect("fixture decodes");
        assert_eq!(
            store.read_chunk(key, None, &mut payload).expect("readable"),
            Some(ChunkRevision::new(1)),
            "{key:?}"
        );
        assert_eq!(payload.cells(), &row.cells[..], "{key:?}");
        assert_eq!(payload.theme(), row.theme, "{key:?}");
    }
    assert_eq!(
        store
            .read_chunk(ChunkKey::new(0, 0, 1), None, &mut payload)
            .err(),
        None
    );
    assert_eq!(
        store.read_chunk(ChunkKey::new(9, 9, 0), None, &mut payload),
        Ok(None),
        "a chunk the old writer never saved is absent and may be generated"
    );
}

#[test]
fn rows_the_planar_world_cannot_hold_refuse_the_import_unless_skipped() {
    let scratch = Scratch::new("nonplanar");
    let original = scratch.legacy_copy();
    let destination = scratch.path("world.redb");
    match legacy::import(
        &original,
        &destination,
        &options(NonPlanarRows::Refuse),
        StoreFaults::new(),
    ) {
        Err(ImportError::NonPlanarRows(keys)) => assert_eq!(keys, vec![ChunkKey::new(0, 0, 1)]),
        other => panic!("expected a refusal: {other:?}"),
    }
    assert!(!destination.exists(), "nothing was written");
    assert!(matches!(
        legacy::import(
            &original,
            &original,
            &options(NonPlanarRows::Skip),
            StoreFaults::new()
        ),
        Err(ImportError::DestinationExists(_))
    ));
}

#[test]
fn a_corrupt_row_fails_the_import_and_stays_as_it_was() {
    let scratch = Scratch::new("corrupt");
    let original = scratch.legacy_copy();
    let key = ChunkKey::new(2, 1, 0);
    let truncated = row_bytes(key)[..1000].to_vec();
    {
        let store = LegacyStore::open_or_create(&original).expect("legacy");
        store
            .write_batch(&[(key, truncated.as_slice())])
            .expect("damage one row");
    }
    let destination = scratch.path("world.redb");
    match legacy::import(
        &original,
        &destination,
        &options(NonPlanarRows::Skip),
        StoreFaults::new(),
    ) {
        Err(ImportError::CorruptRows(rows)) => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].0, key);
        }
        other => panic!("expected the corrupt row to fail the import: {other:?}"),
    }
    assert!(!destination.exists(), "nothing was written");
    let store = LegacyStore::open_or_create(&original).expect("legacy");
    assert!(matches!(
        store.read_chunk(key),
        Err(legacy::LegacyError::Corrupt { .. })
    ));
}

#[test]
fn a_destination_failure_leaves_an_incomplete_store_that_opening_refuses() {
    let scratch = Scratch::new("destination");
    let original = scratch.legacy_copy();
    let destination = scratch.path("world.redb");
    let faults = StoreFaults::new();
    faults.arm(Barrier::BeforeChunkCommit, Action::Fail, 1);
    match legacy::import(
        &original,
        &destination,
        &options(NonPlanarRows::Skip),
        faults,
    ) {
        Err(ImportError::Destination(StoreError::Io { .. })) => {}
        other => panic!("expected the destination failure: {other:?}"),
    }
    assert!(matches!(
        RedbWorldStore::open(&destination, OpenOptionsStore::default()),
        Err(StoreError::Incomplete)
    ));
    // The original still imports into a new destination.
    let again = scratch.path("world-2.redb");
    legacy::import(
        &original,
        &again,
        &options(NonPlanarRows::Skip),
        StoreFaults::new(),
    )
    .expect("imports");
}

const CHILD_SOURCE: &str = "WORLD_STORE_IMPORT_SOURCE";
const CHILD_DESTINATION: &str = "WORLD_STORE_IMPORT_DESTINATION";
const CHILD_KIND: &str = "WORLD_STORE_IMPORT_KIND";

/// The child half of the interruption tests: does nothing unless the parent set the variables.
#[test]
fn import_interruption_child() {
    let (Ok(source), Ok(destination), Ok(kind)) = (
        std::env::var(CHILD_SOURCE),
        std::env::var(CHILD_DESTINATION),
        std::env::var(CHILD_KIND),
    ) else {
        return;
    };
    match kind.as_str() {
        "import" => {
            let faults = StoreFaults::new();
            faults.arm(Barrier::BeforePublishCommit, Action::Abort, 1);
            let _ = legacy::import(
                Path::new(&source),
                Path::new(&destination),
                &options(NonPlanarRows::Skip),
                faults,
            );
        }
        "legacy-writer" => {
            // The old application stopped in the middle of a save: its file is unclean.
            let store = LegacyStore::open_or_create(Path::new(&source)).expect("legacy");
            let row = row_bytes(ChunkKey::new(0, 0, 0));
            store
                .write_batch(&[(ChunkKey::new(5, 5, 0), row.as_slice())])
                .expect("write");
            std::process::abort();
        }
        other => panic!("unknown kind {other}"),
    }
}

fn run_child(kind: &str, source: &Path, destination: &Path) {
    let output = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "import_interruption_child",
            "--exact",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(CHILD_SOURCE, source)
        .env(CHILD_DESTINATION, destination)
        .env(CHILD_KIND, kind)
        .output()
        .expect("child runs");
    assert!(!output.status.success(), "the child was not interrupted");
}

#[test]
fn an_interrupted_import_is_incomplete_and_the_original_imports_again() {
    let scratch = Scratch::new("interrupted");
    let original = scratch.legacy_copy();
    let before = fs::read(&original).expect("bytes");
    let destination = scratch.path("world.redb");
    run_child("import", &original, &destination);
    assert_eq!(fs::read(&original).expect("bytes"), before);
    assert!(matches!(
        RedbWorldStore::open(&destination, OpenOptionsStore::default()),
        Err(StoreError::Incomplete)
    ));
    let again = scratch.path("world-2.redb");
    let report = legacy::import(
        &original,
        &again,
        &options(NonPlanarRows::Skip),
        StoreFaults::new(),
    )
    .expect("imports");
    assert_eq!(report.interpretation.rows_imported, 4);
}

#[test]
fn an_unclean_original_is_read_through_a_repaired_copy() {
    let scratch = Scratch::new("unclean");
    let original = scratch.legacy_copy();
    run_child("legacy-writer", &original, &scratch.path("unused"));
    let before = fs::read(&original).expect("bytes");
    let destination = scratch.path("world.redb");
    let report = legacy::import(
        &original,
        &destination,
        &options(NonPlanarRows::Skip),
        StoreFaults::new(),
    )
    .expect("imports");
    assert_eq!(
        fs::read(&original).expect("bytes"),
        before,
        "the original was not repaired"
    );
    let copy = report.read_repaired_copy.expect("a repaired copy was read");
    assert!(copy.exists());
    // The row the interrupted writer committed before it stopped is part of the import.
    assert_eq!(report.interpretation.rows_imported, 5);
}

//! The legacy codec and store against rows the old writer produced (`fixtures/legacy-v0`).
//!
//! Expected values come from the fixture manifest, which the capture program computed from the
//! in-memory chunks before encoding, so they do not depend on the decoder under test.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use morbid_app::legacy_format;
use redb::{Database, ReadableDatabase, ReadableTableMetadata, TableDefinition};
use spatial_lib::chunk::storage::{ChunkStorage, redb_backend::RedbChunkStorage};
use world_model::{generation::WorldGenerator, spatial::ChunkKey};

const TABLE: TableDefinition<[i32; 3], &[u8]> = TableDefinition::new("spatial_chunks");

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy-v0")
}

struct Row {
    key: ChunkKey,
    bytes: Vec<u8>,
    manifest: toml::Table,
}

fn rows() -> Vec<Row> {
    let text = fs::read_to_string(fixtures().join("manifest.toml")).expect("manifest");
    let manifest: toml::Table = toml::from_str(&text).expect("manifest parses");
    manifest["row"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| {
            let row = row.as_table().expect("row table").clone();
            let key: Vec<i32> = row["key"]
                .as_array()
                .expect("key")
                .iter()
                .map(|v| v.as_integer().expect("integer") as i32)
                .collect();
            let file = row["file"].as_str().expect("file");
            Row {
                key: ChunkKey::new(key[0], key[1], key[2]),
                bytes: fs::read(fixtures().join(file)).expect("row bytes"),
                manifest: row,
            }
        })
        .collect()
}

fn words(value: &toml::Value) -> Vec<u64> {
    value
        .as_array()
        .expect("words")
        .iter()
        .map(|word| {
            u64::from_str_radix(word.as_str().expect("hex").trim_start_matches("0x"), 16)
                .expect("hex word")
        })
        .collect()
}

#[test]
fn every_row_decodes_to_the_values_the_old_writer_recorded() {
    let rows = rows();
    assert_eq!(rows.len(), 5);
    for row in rows {
        let chunk = legacy_format::decode(&row.bytes).expect("decodes");
        let m = &row.manifest;
        let integer = |name: &str| m[name].as_integer().expect(name) as u64;
        let sum = |field: fn(&world_model::cell::WorldCell) -> u64| {
            chunk.cells.iter().map(field).sum::<u64>()
        };
        assert_eq!(chunk.cells.len() as u64, integer("cells"), "{:?}", row.key);
        assert_eq!(
            chunk.metadata.last_simulated,
            m["last_simulated"].as_float().expect("float")
        );
        assert_eq!(u64::from(chunk.metadata.theme.0), integer("theme"));
        assert_eq!(sum(|c| c.elevation() as u64), integer("elevation_sum"));
        assert_eq!(sum(|c| c.fluid_vol() as u64), integer("fluid_volume_sum"));
        assert_eq!(
            sum(|c| c.granular_vol() as u64),
            integer("granular_volume_sum")
        );
        assert_eq!(
            sum(|c| c.surface_state() as u64),
            integer("surface_state_sum")
        );
        let cells: Vec<u64> = chunk.cells.iter().map(|cell| cell.0).collect();
        assert_eq!(cells[..16], words(&m["first_words"])[..]);
        assert_eq!(cells[cells.len() - 4..], words(&m["last_words"])[..]);
    }
}

#[test]
fn re_encoding_reproduces_the_old_writer_bytes() {
    for row in rows() {
        let chunk = legacy_format::decode(&row.bytes).expect("decodes");
        assert_eq!(legacy_format::encode(&chunk), row.bytes, "{:?}", row.key);
    }
}

#[test]
fn truncated_and_foreign_bytes_are_errors_not_chunks() {
    let bytes = &rows()[0].bytes;
    assert!(legacy_format::decode(&bytes[..bytes.len() / 2]).is_err());
    assert!(legacy_format::decode(b"not a chunk").is_err());
}

#[test]
fn the_store_holds_exactly_the_recorded_rows() {
    // Work on a copy: opening the store takes a write transaction.
    let dir = std::env::temp_dir().join(format!("legacy-fixture-store-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch");
    let copy = dir.join("legacy-chunks.redb");
    fs::copy(fixtures().join("legacy-chunks.redb"), &copy).expect("copy store");

    let database = Arc::new(Database::open(&copy).expect("opens"));
    let count = database
        .begin_read()
        .expect("read")
        .open_table(TABLE)
        .expect("table")
        .len()
        .expect("length");
    let storage = RedbChunkStorage::new(database).expect("storage");
    let rows = rows();
    assert_eq!(count, rows.len() as u64);
    for row in rows {
        let stored = storage
            .read_chunk(row.key, |bytes| bytes.to_vec())
            .expect("readable")
            .expect("present");
        assert_eq!(stored, row.bytes, "{:?}", row.key);
    }
    assert!(
        storage
            .read_chunk(ChunkKey::new(9, 9, 0), |_| ())
            .expect("readable")
            .is_none()
    );
    drop(storage);
    fs::remove_dir_all(&dir).expect("remove scratch copy");
}

#[test]
fn generated_rows_match_the_extracted_generator() {
    // Rows the capture program took unchanged from the generator (the README's table); the
    // edited and hand-built rows are not generator output.
    let generated = [
        ChunkKey::new(0, 0, 0),
        ChunkKey::new(-1, -2, 0),
        ChunkKey::new(0, 0, 1),
    ];
    let generator = WorldGenerator::new(morbid_app::STARTUP_SEED);
    let rows = rows();
    for key in generated {
        let row = rows.iter().find(|row| row.key == key).expect("fixture row");
        let chunk = legacy_format::decode(&row.bytes).expect("decodes");
        assert_eq!(chunk.cells, generator.generate_chunk(key).cells, "{key:?}");
    }
}

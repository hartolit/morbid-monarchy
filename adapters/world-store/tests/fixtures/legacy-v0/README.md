# Legacy saved-world fixtures (old writer, v0)

These bytes were produced by the baseline save path, not by a reimplementation of it. Treat them
as immutable. A capture made with changed code is a new fixture set, not an update to this one.
They lived under `crates/morbid-app/tests/` until prompt 05 moved them, unchanged (same SHA-256),
to `world-store`, which now owns the legacy row bytes and their import.

## Provenance

| Item | Value |
| --- | --- |
| Source | Commit `eea8652` (toolchain pin over `a9c1539`); capture program `crates/morbid-app/examples/capture_legacy_fixtures.rs` in commit `58e9648`, which added this directory. The program was deleted when the cell types moved to `world-model` and lost their bitcode derive; it remains in history |
| Writer calls | `bitcode::encode(&CellChunk)` (the derived encoding) then `RedbChunkStorage::write_batch`, the same two calls `runtime::persistence::process_save_queue` made at that commit |
| Toolchain / libraries | rustc 1.99.0 (`b940084d7`), bitcode 0.6.9, redb 4.1.0 (from `Cargo.lock`) |
| Table | `spatial_chunks`, key `[i32; 3]`, value bitcode-encoded `CellChunk` |
| Generator seed | 69 (the value hard-coded in `morbid_app::run`) |
| Reproducibility | Two captures produced byte-identical files, including `legacy-chunks.redb` |

`legacy-chunks.redb` holds all five rows, committed in two batches (rows 1–2, then 3–5) the way
periodic save flushes commit them. `rows/` holds each row's value bytes exactly as stored, so that
codec tests do not need redb. `manifest.toml` lists, per row, values computed from the in-memory
chunk before encoding: field sums, the first 16 and last 4 cell words, and the metadata. It gives
readers an expected result that does not depend on the decoder under test.

| Row file | Key | Content |
| --- | --- | --- |
| `0_0_0.bitcode` | (0, 0, 0) | Generated chunk at the origin. |
| `-1_-2_0.bitcode` | (−1, −2, 0) | Generated chunk with negative key components. |
| `2_1_0.bitcode` | (2, 1, 0) | Generated, then edited with the water, sand, raise-terrain and fire brush rules; `last_simulated = 37.25`. |
| `7_-3_0.bitcode` | (7, −3, 0) | Hand-built: cell 0 is zero, cells 1–11 each set one packed field to its maximum, cells 12–14 are all-ones / low word / high word, the rest are SplitMix64 words; theme 7, `last_simulated = 1234.5`. |
| `0_0_1.bitcode` | (0, 0, 1) | **Synthetic.** The normal focus path only writes `z = 0`; this row exists to test how an importer handles nonzero `z`. |

## SHA-256

| File | SHA-256 |
| --- | --- |
| `legacy-chunks.redb` | `d551015b9e81e6bd676c36273f6e1eda1310def5c7dd47691a59bab44871e0a8` |
| `rows/0_0_0.bitcode` | `3d16312b3cb5144932cd4bc2fb4d42a3dc8bed93f7a0dba0d6aad4089be86736` |
| `rows/-1_-2_0.bitcode` | `349abaf3e21045c8aeed62205528a82d9fc6329b5a7feba5cd22d9b50c9e0428` |
| `rows/2_1_0.bitcode` | `cd2b6d786de3b6334bc4ca82a0c41686c710ba204858948860e6af1de69326d3` |
| `rows/7_-3_0.bitcode` | `95d85b0de84128d30d51236ef7635ef7ac909706cbda433f5f98a90ae80d9080` |
| `rows/0_0_1.bitcode` | `26bba714b34c7ca246a86fab93f5ea3e6b2745d8d20f2c63c102534361c243d6` |

# world-store

The saved world: its byte format, its redb store, fault injection at the storage boundary and
the import of legacy saves. It implements the runtime's storage port
(`world_runtime::store::WorldStore`) over one redb file per world; the residency and checkpoint
owners in `world-runtime` decide what is saved and when, this crate decides how it is encoded and
made durable.

| Module | Owns |
| --- | --- |
| `format` | Version 1 of every row: the header (world, generator, cell-layout and physics identity), immutable chunk revisions, heads, body sets, manifests, the latest pointer and the session marker; little-endian fields, CRC-32C trailers, and the checks a reader applies before it allocates. |
| `store` | `RedbWorldStore`: create, open (read-write or read-only), typed reads that keep absent, corrupt, unsupported and unreadable apart, chunk and body writes in single transactions, atomic publication of a checkpoint, reclamation of what no checkpoint reaches, and reopening the file after an I/O failure. |
| `faults` | `StoreFaults`: I/O errors inside redb's own writes and syncs (`FaultBackend`, a `StorageBackend` around the real file), failures and holds at defined points of the store's operations, and process interruption at those points or at the n-th file write. For tests. |
| `legacy` | The baseline's `spatial_chunks` rows: their bitcode bytes (the one owner of them), the table the baseline writes through `LegacyStore` until prompt 06, and `import`, which reads a legacy file without changing it and writes a new world. |

`src/bin/world-store.rs` is the command line: `create`, `import-legacy`, `inspect` and `verify`,
each naming its store; the recipe is in
[world format](../../docs/reference/world-format.md#operating-a-saved-world-05). Opening a
store for writing first looks at the file read-only, because redb changes a file's bytes when it
opens it for writing: a legacy save or any other file given as a store, and a world whose header,
latest checkpoint or latest body set is damaged or from another revision, is refused unchanged.

The exact bytes and the compatibility policy are in
[world format](../../docs/reference/world-format.md#saved-world-format-version-1-05). The
[legacy fixtures](tests/fixtures/legacy-v0/README.md) are the old writer's rows. Tests run against
real files in temporary directories: the format against hand-placed offsets and damaged rows, the
store's publication rules and injected failures, damaged and foreign worlds refused unchanged,
process interruption at every barrier and inside redb's commits (`tests/store_lifecycle.rs`), and
imports of the old writer's rows, including corrupt, unclean, interrupted and failing ones
(`tests/import.rs`).

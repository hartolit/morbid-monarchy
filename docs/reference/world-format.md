# World representation, residency and saved data

This document owns the physical value representation and snapshot/storage lifecycle. The current
format is described separately from the target envelope. 01 captured legacy fixtures and moved the
cell definition into `world-model`; 05 implemented the new format, its store and the import
([version 1](#saved-world-format-version-1-05)), and the residency and checkpoint owners that use
them. 06 removed the baseline application; the legacy format is now only read, by the import.
07 exercised the whole lifecycle together in `resident-world` and through the packaged client
([qualification](../development/qualification.md)).

## Existing packed cell

Source: `crates/world-model/src/cell.rs`, `WorldCell(pub u64)` (in `monarch-engine` until 01).
`WorldCell::LAYOUT` is the one table of the fields below; every accessor reads it, and its tests
compare it with this table written out by hand. Bit 0 is the low bit of the first 32-bit word;
GPU word order is explicitly low word, high word (`WorldCell::to_words`, `from_words`). Do not
rely on native host byte order for persisted bytes or portable interchange. On the device a cell
is a WGSL `vec2<u32>` (`.x` = word 0), and `gpu-world` generates every field's word, shift and
mask from `WorldCell::LAYOUT`, so the kernels cannot keep a second copy of this table; the
`transfers` suite executes those declarations against cells written by the CPU
([GPU contract](gpu-contract.md#cell-layout-and-authority)).

| Bits | Field | Encoded capacity |
| --- | --- | ---: |
| 0–3 | Terrain material | 4 bits |
| 4–7 | Surface material | 4 bits |
| 8–11 | Granular material | 4 bits |
| 12–14 | Fluid material | 3 bits |
| 15–19 | Visual variants | 5 bits |
| 20–31 | Elevation | 4,095 |
| 32–40 | Fluid volume | 511 |
| 41–44 | Granular volume | 15 |
| 45–53 | Surface state | 511 |
| 54–59 | Terrain state | 63 |
| 60–63 | Compass/momentum flags | 4 bits |

Material IDs and scales retain their existing meanings until changed through a tested decision.

**Elevation scale is physical configuration (decided in 01).** One elevation unit is
`elevation_scale` world units high (default 0.5, supported 0.01–5.0,
`world_model::config::ELEVATION_SCALE`). Bodies collide with, float in and deform the scaled
surface, and the renderer draws the same surface, so the scale is not a display setting. Cells
store integer units and never change when it does. The runtime applies a change at a tick
boundary, as a configuration command, and records the scale with the configuration in every
checkpoint; a world reopened with another scale is a different physical world, not a restyle.
The removed baseline wrote `GlobalPhysicsConfig::elevation_scale` immediately from its developer
UI and did not save it; the native client sends the change as a command (06).
Chunks have 64×64 cells. World keys have three signed components, and the active 2.5D view uses a
planar mapping. Source at `a9c1539` shows that mapping: the baseline's grid-attached observer set
the focus to `(x, −z, 0)` from its Bevy translation (`sync_world_focus`), so keys it wrote have
`z = 0`, as the native client's observer does (`morbid-app` `observer`). A cell's `y` is the renderer's `−z` (also used by
`spherical.rs` and the render chunk transforms). The redb key and the generator seed still include
`z`. `ChunkKey::to_ivec2` returns `(x, y)`; the misleading "XZ plane" comment was corrected when
the key moved to `world_model::spatial` in 01. The two render-to-cell conversions are named there:
`nearest_cell` rounds (body centres) and `quad_cell` floors (picking, brushes, the observer's
floor). The old-writer fixtures include a synthetic `z = 1` row; import records how it handles
rows with nonzero `z`. New GPU slots use local indices plus absolute-key metadata, never float
absolute-world IDs.

Keep saved packed words distinct from GPU scratch, padding and handles. A cell-format change needs
an explicit version and conversion. Invalid IDs, lengths, overflows or non-finite body values are
rejected at the owning input boundary.

## Legacy storage facts

The baseline's redb table is `spatial_chunks`, keyed by `[i32; 3]`. Its values are the derived
bitcode encoding of `CellChunk { cells, metadata: { last_simulated: f64, theme: u8 } }`. Since 05
`world_store::legacy` is the one owner of those bytes and the table, tested to re-encode the
[old-writer fixtures](../../adapters/world-store/tests/fixtures/legacy-v0/README.md) byte for
byte; since 06 it only reads them, for the import, because the writer was removed with the
baseline. A legacy file has no versioned world or checkpoint envelope. The baseline used seed 69
in code. Its `last_simulated` metadata holds session-relative elapsed seconds at load time, not
proof of a consistent world snapshot or a portable clock. It wrote only chunks evicted from its
LRU cache, by the periodic save batch or the exit flush, with no dirty flag, so a legacy store
mixes modified chunks with unmodified generated ones and can miss changes that were still
resident or cached when the process stopped. Preserve original bytes for any import
investigation.

A database/table error or undecodable existing row is different from a key that does not exist.
Only a genuine absent key permits deterministic terrain generation. A corrupt/unsupported row must
be reported and preserved; generation cannot silently replace it.

## Target saved-world envelope

Use a versioned world header and immutable chunk/body revisions with a checkpoint manifest. The
header records world identity, generator version/seed, cell/rule format version, coordinate/scale
convention and supported physical configuration identity. The manifest records checkpoint ID,
completed logical tick, active/suspended state, required chunk/body revisions and integrity data.
The storage owner defines exact bytes, bounds and upgrade policy with golden fixtures in 05.

Version semantic data, not driver-specific buffers. Do not save raw ECS IDs, GPU handles, device
addresses or uninitialized struct padding. Save body attributes/state needed to resume the supported
sphere behavior, control selection where meaningful, and any persistent accumulators. A checkpoint
that saves terrain but silently respawns every sphere is not the complete-world checkpoint promised
by this transition.

Use typed counts and checksums/integrity metadata appropriate to detecting accidental corruption;
no cryptographic security claim is required. Validate before large allocation. Unsupported future
versions are refused without modifying the store.

## Saved-world format, version 1 (05)

One redb file holds one world. `world_store::format` owns the exact bytes; its module
documentation lists every field's offset, and its tests check them against values placed by hand.
Every row is an envelope of a four-byte magic, a `u16` format version (1), a reserved `u16`, the
body, and a CRC-32C (Castagnoli) of everything before it, little-endian throughout. A reader checks
the length against the envelope, then the checksum (a mismatch is corruption), then the magic,
then the version (any other is unsupported, and the row is left as it is), then the exact length
the version defines, and only then allocates or decodes. The envelope and the checksum are the
same in every version, so an unsupported row is told apart from a damaged one.

| Table | Key | Row | Bytes |
| --- | --- | --- | ---: |
| `world` | `"header"` | world identity, generator revision and seed, rule seed, cell format revision and layout fingerprint, physics revision and fingerprint, chunk edge 64, axis convention 1, origin (created, or legacy import with the supplied seed and scale and the rows imported and refused) | 92 |
| `world` | `"latest"` | the latest published checkpoint | 20 |
| `world` | `"session"` | whether the last writing session ended in order | 20 |
| `chunks` | `(x, y, z, revision)` | one immutable chunk revision: its key and revision again, theme, 4,096 cells as the packed `u64` | 32,812 |
| `heads` | `(x, y, z)` | the key's published revision and the checkpoint that published it | 20 |
| `bodies` | checkpoint | the body slots (generation and state) and every live body's resume state (`world_model::resume`) | `28 + 8 S + 136 B` |
| `checkpoints` | checkpoint | the manifest: completed tick, rules, ambient temperature, elevation scale, window, saved keys, live bodies, the session epoch, keys moved, the body set's checksum | 92 |

Nothing driver-specific is saved: no GPU record, padding, slot index, handle beyond a body's
semantic slot and generation, or ECS identity. Floats are stored as their IEEE-754 bits and checked
against the model's ranges when read; out of range is corruption, never a clamp. Revision 0 means
"generated, never saved" and is never written. A row repeats its key, so a row under the wrong key
is corrupt.

*Compatibility policy.* A header whose cell format revision or layout fingerprint
(`world_model::identity`), physics revision or fingerprint, or generator revision differs from this
build's is refused as unsupported before anything else is read, and the file is not modified. No
conversion exists yet: a different layout reads other fields out of the same bits, and other rules
or body tuning make it a different physical world. The fingerprints are pinned by tests, so
changing a listed rule or body constant fails until its revision is raised here and in
`world_model::identity`. A row of another format version is refused the same way.

*Publication.* Chunk revisions and a checkpoint's body set are written ahead in their own
transactions (`Durability::Immediate`, redb's default: durable when the commit returns). One
transaction then checks that the checkpoint follows the latest, that its body set exists, that
every revision it names exists and is newer than its key's head (an older save finishing later is
refused: `Conflict::HeadNotNewer`), moves those heads, and writes the manifest and the latest
pointer. A crash leaves the previous complete checkpoint or the new one. A retry of an operation
whose outcome was unknown (a failed sync may still have reached the file) succeeds when it is the
same operation and is refused otherwise. A second transaction removes what the publication
superseded; the session marker makes the next writing session remove whatever an interrupted one
left unpublished before it writes anything (`WorldStore::begin_session`).

*Failures.* Reads return found, absent (only then may terrain be generated), corrupt, unsupported
or I/O failure. A redb handle refuses every transaction after an I/O error, so the store reopens
the file (redb repairs it) before the next operation; a failed write changes nothing and can be
retried. `world_store::faults` injects I/O errors under redb, failures and holds at defined points,
and process interruption for the tests.

*Import.* `world_store::legacy::import` opens the legacy file read-only (one the old application
left unclean is copied, and the copy repaired and read), decodes and checks every row before
writing, refuses a corrupt row (the import fails, nothing is published) and a row with `z ≠ 0`
unless asked to leave such rows out, writes each row as revision 1 of a new store, reads every row
back, and only then publishes the first checkpoint. The supplied seed and elevation scale are
recorded as the import's interpretation in the header. An import that stops earlier leaves an
incomplete store that opening refuses. The original is never written.

### Operating a saved world (05)

Since 05 these commands work on version-1 stores. Since 06 the native client opens, creates and
imports the same stores from its start screen (`<name>.world` in its worlds directory; see the
[package README](../../apps/morbid/README.md)); these commands remain for scripts and inspection.

```bash
cargo run --locked --release -p world-store -- create --store WORLD.redb --seed 7 --radius 2
cargo run --locked --release -p world-store -- import-legacy --legacy COPY.redb --store WORLD.redb --seed 69 --elevation-scale 0.5 --skip-nonplanar-rows
cargo run --locked --release -p world-store -- inspect --store WORLD.redb
cargo run --locked --release -p world-store -- verify --store WORLD.redb
cargo evidence session --store WORLD.redb --adapter ADAPTER_ID --ticks 600 --edit raise,10,10
```

- `create` and `import-legacy` refuse an existing destination. A legacy file records neither seed
  nor elevation scale, so the import requires both and reports and records them as its
  interpretation; `--skip-nonplanar-rows` leaves out rows with `z ≠ 0`, which are refused
  otherwise. Import from a copy while the old application might be running: the importer never
  writes the original, and copies it to repair it when the old application left it unclean.
- `inspect` and `verify` open read-only and leave the file byte for byte. `verify` reads and
  checks every published row and the latest body set.
- `session` (the evidence runner's headless consumer) opens the latest checkpoint as the next
  session epoch, runs it by wall time, applies the edits once the window has settled, and closes
  in order: admission closes, the in-flight ticks complete, and a checkpoint of the last
  completed tick is published (after a save already under way, if that one holds an earlier
  tick). It passes only when that checkpoint was published, all captured revisions are durable
  and the workers finished. Running it again reopens that checkpoint.
- A session that did not close (killed, timed out, device lost) leaves the store marked unclean.
  The next session to open it reclaims the revisions and body sets no checkpoint reached, reports
  that it did, and starts from the latest complete checkpoint; changes after it are lost, as the
  loss report of a lost device says (its checkpoint, tick and ticks lost). Nothing is replayed.
- Every one of these opens only the file it is given. A file that is not a version-1 world (a
  legacy save, another redb file, anything else), and a version-1 world whose header, latest
  checkpoint or latest body set is damaged or from another revision, is refused before it is
  opened for writing, unchanged (the client opens worlds the same way). A damaged chunk row does
  not refuse the world: that chunk stays out, its row byte for byte.

## Residency state machine

`absent -> load/generate pending -> CPU payload ready -> upload reserved -> GPU resident`

From GPU residence:

`resident -> freeze/capture requested -> independent snapshot captured -> source slot retireable`

The captured snapshot then becomes CPU/readback work and a durable or retained-cache revision.
A live resident record contains identity, slot, generation and revision, not a permanent cell copy.
Dirty means changed since its last acknowledged durable/captured revision, not merely “the CPU
wrote something.” GPU changes must advance dirty/revision metadata too.

A load response is accepted only when its key, request generation and wanted revision still match.
A captured newer revision or dirty retained snapshot takes precedence over an older disk read.
Movement out of view does not cancel the obligation to preserve modified state. Bodies touching a
chunk follow the declared residency/suspension policy; topology changes cannot silently delete them.

Use bounded byte and concurrency budgets for pending loads, uploads, captures, retries and optional
clean caching. Under pressure, pause new transitions or refuse a resize. Do not overwrite a dirty
slot to keep the camera moving. Expose the pressure instead of allowing a hidden memory leak.

## Checkpoint consistency

An autosave must include dirty chunks that remain resident, not only evictions. It must also include
body state and simulation configuration. Choose a real consistent capture at completed tick T:
copy required live GPU state into an independent checkpoint image on the ordered queue, then allow
simulation to continue while that image drains in bounded pieces. Allocate this image within the
complete memory plan before requesting capture.

Alternatively, use a proved copy-on-write/versioned scheme with equivalent consistency and a
measured reason. Do not independently read chunks over many changing ticks and label the result a
single-tick snapshot. Explicit maintenance may pause at a boundary when necessary, with a visible
outcome; routine saving should not impose a synchronous CPU/GPU frame rendezvous.

Previously inactive chunks remain frozen at their recorded state. A manifest can refer to those
immutable revisions alongside the active state captured at T because the inactive regions have
not continued simulating. On reactivation, preserve that policy and wake dependencies. Do not invent
unloaded-time catch-up as a side effect of checkpoint loading.

Persist prepared chunk/body revisions in bounded transactions. Publish the checkpoint manifest only
when every referenced payload is successfully durable under the chosen redb durability setting.
The publication is atomic: a crash exposes the previous complete manifest or the new complete one,
never a manifest pointing at an uncommitted payload. Partial prepared data is not an accepted
checkpoint and can be reclaimed by bounded recovery after reachability is established.

Revision ordering prevents a slow older save from replacing a newer revision. Retain failed write
payloads or their independent snapshot source until retry/recovery resolves the obligation. A save
task returns success/failure and identity; logging a failure and returning `()` is insufficient.

## Exit, interruption and device loss

Orderly exit closes new commands, chooses/captures a completed boundary, drains owned transfers,
commits the complete checkpoint and reports the result. Track in-flight saves as well as queued
ones. A timeout/failure is visible and preserves the last complete checkpoint; do not print “safe
to terminate” while dirty resident state was never captured.

A process crash can lose changes after the last published checkpoint. Device loss can lose
unsnapshotted GPU state. State that loss boundary plainly. Recovery reconstructs from the last
validated complete checkpoint with a new session epoch; it does not claim exact replay of commands
that were never saved. Ignore callbacks from the old epoch. Successful recovery and orderly shutdown
are different evidence cases: `resident-world` step 6 runs them in one trace (an orderly close,
a device lost before a save's payloads reached the CPU, and a process ending inside its save
before and after the publication commits), and each reopen finds exactly the checkpoint the
boundary names, with its bodies and configuration (07).

## Legacy import and replacement

Never test import against a user's only database. Capture fixtures using the old writer and
record their source/version. Read the original without destructive migration, validate every imported
key/payload, and write a separate new-format store. A legacy file lacking world metadata requires an
explicit supplied generator/scale interpretation, shown in the import report; do not silently assume
seed 69 was used for every user world.

Only publish/select the new store after complete validation succeeds. Keep the original recoverable.
Tests cover malformed/truncated bytes, unsupported keys, mixed revisions, failed destination writes
and interrupted import. Importing a legacy snapshot cannot recover changes that the old app never
saved. Unsupported legacy cases are preserved with a concrete refusal, not replaced by fresh terrain:
a save with an undecodable row is refused unchanged by `world-store import-legacy` and by the
client's start screen, which lists no world for it (the `worlds` script, 07).

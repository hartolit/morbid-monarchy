# World representation, residency and saved data

This document owns the physical value representation and snapshot/storage lifecycle. The current
format is described separately from the target envelope. 01 captures legacy fixtures; 05 implements
the new writer/import path. No new saved-world reader is present in this documentation delivery.

## Existing packed cell

Source: `crates/monarch-engine/src/core/world/cell.rs`, `WorldCell(pub u64)`.
Bit 0 is the low bit of the first 32-bit word; GPU word order is explicitly low word, high word.
Do not rely on native host byte order for persisted bytes or portable interchange.

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
Chunks have 64×64 cells. CPU world keys in the baseline have three signed components, and the
active 2.5D view uses a planar mapping. Source at `a9c1539` shows that mapping. A grid-attached
observer sets the focus to `(x, −z, 0)` from its Bevy translation (`sync_world_focus`), so keys
written through the normal path have `z = 0`. A cell's `y` is the renderer's `−z` (also used by
`spherical.rs` and the render chunk transforms). The redb key and the generator seed still include
`z`. The comment on `ChunkKey::to_ivec2` says "XZ plane", but the code returns `(x, y)`; do not
rely on the comment. 01 confirms this with old-writer fixtures. Import records how it handles
rows with nonzero `z`. New GPU slots use local indices plus absolute-key metadata, never float
absolute-world IDs.

Keep saved packed words distinct from GPU scratch, padding and handles. A cell-format change needs
an explicit version and conversion. Invalid IDs, lengths, overflows or non-finite body values are
rejected at the owning input boundary.

## Legacy storage facts

The baseline redb table is `spatial_chunks`, keyed by `[i32; 3]`, with bitcode-encoded `CellChunk`
bytes. There is no complete versioned world/checkpoint envelope. The application uses startup seed
69 in code. Existing `last_simulated` metadata holds session-relative elapsed seconds at load time.
It is not proof of a consistent world snapshot or a portable simulation-clock contract. Only
chunks evicted from the LRU cache are ever written, either by the periodic save batch or by the
exit flush, and there is no dirty flag. A legacy store therefore mixes modified chunks with
unmodified generated ones, and it can be missing changes that were still resident or cached when
the process stopped. Preserve original bytes for any import investigation.

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
are different evidence cases.

## Legacy import and replacement

Never test import against a user's only database. Capture fixtures using the old writer and
record their source/version. Read the original without destructive migration, validate every imported
key/payload, and write a separate new-format store. A legacy file lacking world metadata requires an
explicit supplied generator/scale interpretation, shown in the import report; do not silently assume
seed 69 was used for every user world.

Only publish/select the new store after complete validation succeeds. Keep the original recoverable.
Tests cover malformed/truncated bytes, unsupported keys, mixed revisions, failed destination writes
and interrupted import. Importing a legacy snapshot cannot recover changes that the old app never
saved. Unsupported legacy cases are preserved with a concrete refusal, not replaced by fresh terrain.

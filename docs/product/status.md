# Current status

**Snapshot:** source review of the supplied archive on 2026-10-10. No application build or hardware
execution was performed for this documentation package. Update this page when implementation
produces evidence; do not convert target architecture into past-tense claims.

## Supplied implementation

| Area | Observed implementation | Qualification |
| --- | --- | --- |
| Workspace | `crates/monarch-engine`, `crates/morbid-app`, `crates/spatial-lib`; Bevy 0.19 requested by manifest. | Source inspected; dependency resolution/build unverified. |
| Terrain | Eight-byte packed columns, double-buffered CPU automaton, wake/halo tracking, Rayon traversal. | Source and ten simulation tests inspected; not run. |
| Bodies | CPU sphere integration, terrain contact/deformation, fluid displacement, all-pairs sphere checks. | Twelve body tests present; not run. |
| Material contracts | Column mixing, phase changes and capacity helpers. | Five cell tests and one apportionment test present; not run. |
| Rendering | Bevy material shaders; four layers share one cell buffer. Dirty updates copy the whole cell array. | Source inspected; no GPU capture or frame-time measurement. |
| Streaming/storage | Toroidal CPU grid, retained active/cached chunks, asynchronous redb/bitcode operations. | Source inspected; failure/restart safety not qualified. |
| Interaction | Brushes, metal/wood sphere spawning, possession, attract/lift, camera and tuning UI. | Source inspected; no interactive run. |
| Event path | Event types/queue exist; the app consumer in `hooks.rs` is commented out and inspected fire/biology callbacks do not emit events. | Not an implemented gameplay notification system. |

The source audit and exact test-name inventory are in the active sprint's
[source audit](../development/virtual-office/gpu-transition/source-audit.md).
The total is 28 `#[test]` functions in the supplied Rust source, not 28 passing tests.

## Established transition gaps

The CPU copies/traverses the complete active grid despite sleeping cells and maintains additional
active-chunk/render payload copies. Terrain advances on a fixed clock; bodies subdivide frame delta
separately. Their registration does not define an explicit terrain/body order. Current save tasks
log failed writes without returning their payload for retry; read/decode errors can fall through to
generation. Exit handling only flushes the current queue, not a complete active-world checkpoint.
These are source observations, not claims about incidents on the user's machine.

The archive has no committed lockfile, toolchain pin, CI workflow or evidence runner. `Cargo.lock`
is ignored. Old rule documents name toolchain versions but are not executable pins. None of the
new `cargo evidence` commands exist in this baseline.

## Target work not yet implemented

GPU resource authority, the native renderer/client, generation-safe transfers, checkpoint protocol,
reproducible benchmarks, mandatory shader execution tests and repository contract tooling are all
pending. The documentation package supplies their design and assignments only.

## Evidence ledger

| Lane | State | Evidence |
| --- | --- | --- |
| Documentation/source audit | Reviewed | Uploaded archive identity and source locations in the source audit. |
| Rust compilation, linting and tests | Not run | No result claimed. |
| GPU correctness and buffer layout | Not run | No device selected. |
| Primary hardware performance | Not measured | Budget values are proposed qualification targets. |
| Native interactive scenario | Not run | Current app and target app both unqualified here. |
| Fault recovery and legacy save import | Not run | Must operate on temporary stores/copies. |
| Secondary GPU / Windows / other platforms | Unqualified | No compatibility claim. |

After each completed stage, replace affected rows with the tested commit, command, environment and
result. Keep detailed logs in artifacts, not this page. At sprint removal, replace the temporary
source-audit link with the retained evidence location or a compact permanent baseline summary.

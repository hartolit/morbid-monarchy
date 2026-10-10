# Current status

**Snapshot:** prompt 00, 2026-10-10. This is a source review of checkout `a9c1539` on branch
`experimental` (clean tree; source last changed in `e1fa1d2`), plus documentation adoption. No
application build, test run or hardware execution was performed. Update this page when
implementation produces evidence; do not convert target architecture into past-tense claims.

## Current implementation

| Area | Observed implementation | Qualification |
| --- | --- | --- |
| Workspace | `crates/monarch-engine`, `crates/morbid-app`, `crates/spatial-lib`. Manifests request Bevy 0.19 and `bevy_egui` 0.41.0. An untracked, git-ignored `Cargo.lock` resolves Bevy 0.19.0 and wgpu 29.0.4. | Source inspected; dependency resolution/build unverified. |
| Terrain | Eight-byte packed columns, double-buffered CPU automaton, wake/halo tracking, Rayon traversal. | Source and ten simulation tests inspected; not run. |
| Bodies | CPU sphere integration, terrain contact/deformation, fluid displacement, all-pairs sphere checks. | Twelve body tests present; not run. |
| Material contracts | Column mixing, phase changes and capacity helpers. | Five cell tests and one apportionment test present; not run. |
| Rendering | Bevy material shaders. Four layer entities per active chunk share one cell buffer, and dirty updates copy the whole cell array. | Source inspected; no GPU capture or frame-time measurement. |
| Streaming/storage | Toroidal CPU grid. Departing chunks wait in an LRU cache and are saved only when evicted, with no dirty tracking. redb/bitcode operations run asynchronously. | Source inspected; failure/restart safety not qualified. |
| Interaction | Brushes, metal/wood sphere spawning, possession, attract/lift, observer walk/noclip, camera, and simulation/lighting/resize UI. Brush picking, observer walking and statistics read the CPU grid directly. | Source inspected; no interactive run. |
| Event path | Event types/queue exist; the app consumer in `hooks.rs` is commented out and inspected fire/biology callbacks do not emit events. | Not an implemented gameplay notification system. |

The active sprint's [source audit](../development/virtual-office/gpu-transition/source-audit.md)
records the exact symbols, the 00 reconciliation with this checkout and the test inventory. The
total is 28 `#[test]` functions in the Rust source (none in `spatial-lib`), not 28 passing tests.

## Established transition gaps

The CPU copies/traverses the complete active grid despite sleeping cells and maintains additional
active-chunk/render payload copies. Terrain advances on a fixed clock; bodies subdivide frame delta
separately. Their registration does not define an explicit terrain/body order. Current save tasks
log failed writes without returning their payload for retry; read/decode errors can fall through to
generation. A chunk requested again while its save is still queued or in flight is read from the
store without checking that pending payload. Exit handling flushes only the save queue; it does not
capture active or cached chunks or in-flight saves. These are source observations, not claims about
incidents on the user's machine.

The checkout has no tracked lockfile, toolchain pin, CI workflow or evidence runner. `.gitignore`
ignores `Cargo.lock` and `world_data/`. The local toolchain is stable rustc 1.99.0. The retired rule
documents named 1.96.1 stable and 1.98.0 nightly, but those were never tested pins. None of the
`cargo evidence` commands exist yet.

## User data

`world_data/save.redb` (8,074,563,584 bytes at 00, git-ignored) is the user's only world database.
On startup the baseline opens or creates it relative to the working directory. Experiments,
fixtures and benchmarks use temporary stores and a scratch working directory; they never open this
file. See the [development guide](../guides/development.md#baseline-versus-target).

## Target work not yet implemented

GPU resource authority, the native renderer/client, generation-safe transfers, checkpoint protocol,
reproducible benchmarks, mandatory shader execution tests and repository contract tooling are all
pending. 00 adopted their design and assignments; none of them is implemented.

## Evidence ledger

| Lane | State | Evidence |
| --- | --- | --- |
| Documentation/source audit | Reconciled (00) | Audit symbols and the 28-test inventory re-checked against `a9c1539`; differences recorded in the source audit. Local-link and whitespace checks on the adoption diff are listed in the sprint handoff. |
| Rust compilation, linting and tests | Not run | No result claimed. |
| GPU correctness and buffer layout | Not run | No device selected. |
| Primary hardware performance | Not measured | Budget values are proposed qualification targets. |
| Native interactive scenario | Not run | Current app and target app both unqualified here. |
| Fault recovery and legacy save import | Not run | Must operate on temporary stores/copies. |
| Secondary GPU / Windows / other platforms | Unqualified | No compatibility claim. |

After each completed stage, replace affected rows with the tested commit, command, environment and
result. Keep detailed logs in artifacts, not this page. At sprint removal, replace the temporary
source-audit link with the retained evidence location or a compact permanent baseline summary.

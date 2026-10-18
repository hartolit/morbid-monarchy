# Current status

**Snapshot:** prompt 04, 2026-10-18, branch `experimental`. The baseline application still owns the
world on the CPU; 01 measured it, pinned its build, extracted the framework-free model it now
uses and added the evidence runner. 02 added the GPU boundary: a runtime and a GPU owner that
execute ordered brush edits, column queries and chunk transfers on the device. 03 runs every
material rule family on the GPU owner, with exact accounting and sparse tile scheduling. 04 puts
bodies on the GPU owner on the same fixed clock, coupled to its terrain and liquids, with
GPU-resolved rays, every body command and a CPU consumer of body observations, all tested
headlessly on the RTX 5070 Ti. The baseline application does not use the new owner.
Update this page when implementation produces evidence; do not convert target architecture into
past-tense claims.

## Current implementation

| Area | Observed implementation | Qualification |
| --- | --- | --- |
| Workspace | `crates/{monarch-engine,morbid-app,spatial-lib,world-model,world-runtime}`, `adapters/gpu-world` and `tools/evidence`. Rust 1.99.0 is pinned in `rust-toolchain.toml`; `Cargo.lock` is tracked (Bevy 0.19.0, `bevy_egui` 0.41.0, egui 0.35.0, wgpu 29.0.4, winit 0.30.13, redb 4.1.0, bitcode 0.6.9). | Builds with `--locked`; `check static` passes, with 3 recorded Clippy exceptions (7 diagnostics in `morbid-app`, removed in 06). Six locked crates were updated past RustSec advisories after the baseline was measured; four advisories without a compatible fix are accepted in `deny.toml` with reasons. |
| World model | `world-model` owns packed cells and their column rules, materials, the automaton's rules (`rules`: switches, cadences, tuning constants, wake levels, keyed hashes and shuffles), chunk keys, units, supported settings, body presets, descriptions and tuning (since 04 the baseline engine reads its sphere constants from `world_model::body::tuning`), commands (including rays and targets, the column query, configuration and totals), observations (body samples and counters, ray hits) and generation. Engine, application, runtime, GPU owner and runner import it. Since 03 the quench converts only what fits under the elevation ceiling. | 38 tests, including hand-written layout goldens, the baseline hash written out by hand, independent ceiling fixtures and the substep and apportionment rules; legacy fixtures decode and re-encode byte for byte. |
| Runtime | `world-runtime`: session, command and tick identities; the `PhysicalWorld` port with per-tick material and body accounting; bounded receipts (retry applies once, reuse conflicts, acknowledged identities expire); the outcome ledger separating important results from counted cosmetic drops and counting unbalanced ticks; the body monitor (since 04: sample selection, per-body watermarks, a bounded event ring that defers instead of overwriting); the fixed, reconfigurable clock passed to the live world; tick-scheduled commands; a standalone `bevy_ecs` runtime. Used headlessly by the GPU suites and `resident-world`; the baseline does not use it. | 31 tests of its rules; exercised with the GPU owner in the `transfers`, `cells` and `bodies` suites. |
| GPU world | `gpu-world`: explicit adapter selection, requested limits, one submission timeline (≤ 3 in flight), checked resource plans up to radius 32, generated layouts, material tables and rule constants, assembled WGSL validated offline, chunk slots, ordered brush edits, column queries, configuration and totals commands, pooled upload/readback staging, resize with device-to-device migration, loss, fault and stop handling. Since 03 every tick runs the material phases (propose, resolve, react, commit) over the tiles its activity scheduling selects on the device, sparse or dense. Since 04 it owns bodies: a generation-checked slot table, device-resident state, substeps on the tick's clock, complete hashed bins, a per-body workgroup Jacobi solver, terrain contact and shared deformation under per-cell claims, buoyancy and displacement, suspension at the residency edge, GPU-resolved rays and every body command, and selected body samples. | 31 tests. `transfers` (28 cases), `cells` (31 cases) and `bodies` (32 cases) pass on the RTX 5070 Ti at 04. Measured: an idle radius-6 window costs 27 µs of device time per tick, a fully active one 280 µs ([material phases](../reference/performance-budgets.md#material-phases-03)); the legacy qualification body workload's body pass takes 0.63 ms ([bodies](../reference/performance-budgets.md#bodies-04)). |
| Terrain | Baseline: eight-byte packed columns, double-buffered CPU automaton, wake/halo tracking, Rayon traversal; since 03 it reads its constants and hashes from `world_model::rules`, with identical results. GPU (03): the same rules over the same cells as WGSL kernels, every transfer proposed once and accepted against the receiver's capacity; idle tiles are neither visited nor copied. | 10 baseline simulation tests pass. The CPU reference in `monarchy-evidence` equals the baseline engine cell for cell on its fixtures, and the GPU equals the reference tick for tick in the `cells` suite on the RTX 5070 Ti. Measured: the baseline still copies the whole array every tick; the GPU path visits no tile in an idle world. |
| Bodies | Baseline: CPU sphere integration, terrain contact/deformation, fluid displacement, all-pairs sphere checks. GPU (04): the same behavior on the device in fixed substeps, with hashed bins instead of all pairs, swept contacts, a speed cap of 600 units/s and exact material accounting. | 12 baseline body tests pass, and their 12 intentions pass as GPU cases. Measured on the frozen legacy workloads: body time per tick 5.32 ms (CPU, 64 spheres) and 16.99 ms (256) in the baseline, 0.63 ms and 0.98 ms of device time on the GPU owner. |
| Material contracts | Column mixing, phase changes and capacity helpers in `world-model`; the per-operation conservation, sinks and capacity policy in the [GPU contract](../reference/gpu-contract.md#material-accounting-03); since 04 the bodies' accounting and the compaction sink ([bodies](../reference/gpu-contract.md#bodies-04)). | 5 baseline cell tests, 3 ceiling tests and 1 apportionment test pass; the WGSL rules equal them on 15,488 boundary columns on the device, and the GPU apportionment equals `world-model`'s. Every GPU tick reports exact 64-bit accounting of cells and bodies that the ledger checks. |
| Rendering | Bevy material shaders. Four layer entities per active chunk (676 at radius 6, counted from the measured active chunks, not from draw calls) share one cell buffer; a dirty grid uploads the whole array. | Measured GPU passes and frame times (see the baseline below); visual reference frames and the lighting/quality defaults are recorded for 06. |
| Streaming/storage | Toroidal CPU grid. Departing chunks wait in an LRU cache and are saved only when evicted, with no dirty tracking. redb operations run asynchronously; `legacy_format` owns the row bytes. | Fixture-tested codec. Failure/restart safety not qualified. |
| Interaction | Baseline: brushes, metal/wood sphere spawning, possession, attract/lift, observer walk/noclip, camera, and simulation/lighting/resize UI. UI ranges come from `world_model::config`. GPU (04): every brush, spawn, removal, drive, jump, attract and lift as commands, at a column or at a GPU-resolved ray, through the runtime's receipts. | Baseline exercised through scripted scenarios. GPU commands, rays and retries tested headlessly; no interactive session was qualified. |
| Event path | In the baseline, event types and an unbounded queue exist; nothing consumes them. The GPU path has the bounded replacement: command outcomes and residency results with reserved storage, a per-tick cosmetic event buffer whose drops are counted, and since 04 body samples and per-body counters consumed by `world-runtime`'s body monitor, whose events are deferred, never dropped, under pressure. Contacts, landings, craters, splashes and immersion are the producers. | Delivery under a saturated ring, delayed and reordered samples and slot reuse is tested on the GPU (`observation-delivery`), and `resident-world`'s consumer reacts to it; not yet a gameplay notification system. |

The sprint's [source audit](../development/virtual-office/gpu-transition/source-audit.md) records
the exact symbols and what 01 found while measuring.

## Measured baseline

On the nominated machine (Ryzen 9 5950X, RTX 5070 Ti, Vulkan, NVIDIA 615.71.09, KDE Wayland) at
`6e108d6`, five valid runs per case: the qualification profile (radius 6, 64 bodies, 1920×1080)
misses its p95 and p99 targets in body-contact (p95 18.7 ms) and coupled (p95 28.6 ms, p99
32.1 ms) and meets them in idle, flow, mixed and streaming. The scale profile (radius 16, 256
bodies) falls behind real time in every active case (17–30 ticks/s). The tables, conditions and
conclusions are in [performance budgets](../reference/performance-budgets.md#measured-legacy-baseline-01).

## Established transition gaps

The CPU copies and traverses the complete active grid despite sleeping cells and keeps additional
active-chunk and render payload copies (30.9 MiB retained at radius 6, 163 MiB at radius 16).
In the baseline, terrain advances on a fixed clock while bodies subdivide frame time separately,
and their registration defines no terrain/body order (the GPU owner runs both on the tick, bodies
after the material commit). Save tasks log failed writes without retaining their
payload; read and decode errors fall through to generation. A chunk requested again while its save
is queued or in flight is read from the store without checking the pending payload. The exit flush
covers only the save queue, not active or cached chunks or in-flight saves. Chunks that stream in
after startup are not woken. These are source observations and measurements on scratch worlds,
not claims about incidents in the user's world. The GPU owner (03) does not have the first or the
last of these gaps — it visits no tile of an idle world and wakes chunks as they activate — but no
application uses it yet.

## User data

`world_data/save.redb` (git-ignored) is the user's only world database: 8,074,563,584 bytes at
00, and 8,237,617,152 bytes, modified 2026-10-11 14:24 local time, when 01 finished. That write
happened while no 01 work was running; 01 never opened the file. It was unchanged when 02 and 03
finished (same size, same modification time); neither opened it. Between the end of 03
(2026-10-17 17:59 local time) and the start of 04 (18:41) it was written again outside the
sprint's work: 8,266,977,280 bytes, modified 2026-10-17 18:39:53 local time, which is what 04
found; no 04 command opened it, and it was unchanged when 04 finished. On startup the baseline opens or
creates it relative to the working directory. Experiments, fixtures and benchmarks use temporary
stores and a scratch working directory. See the
[development guide](../guides/development.md#baseline-versus-target).

## Target work not yet implemented

The residency policy and checkpoint protocol (05, including persisted suspended bodies), and the
native renderer and client (06) are pending; `resident-world` runs its first three steps and
fails the rest until 05 and 07. The GPU owner, its transfers and shader validation exist since
02; the GPU material phases, their accounting and sparse scheduling since 03; bodies, rays and
body observations since 04.

## Evidence ledger

| Lane | State | Evidence |
| --- | --- | --- |
| Documentation | Passed (01) | `cargo evidence check docs`: links and anchors, owners, sprint structure, retired rules, package READMEs and their naming in the architecture. |
| Rust compilation, linting and dependency policy | Passed (02) | `cargo evidence check static`: rustfmt, three feature combinations, Clippy against 3 recorded exceptions (7 diagnostics, removal in 06), rustdoc with warnings denied, naga validation of the assembled shaders, `cargo deny`, `cargo machete`, dependency-direction rules per package, pins and CI. |
| CPU tests | Passed (04) | `cargo evidence test cpu`: 184 tests of the production owners, checked against 133 registered behaviors (28 baseline). |
| GPU correctness and buffer layout | Passed (04, `transfers`, `cells` and `bodies`) | `cargo evidence test gpu --suite transfers --adapter vulkan-10de-2c05` (28 cases), `--suite cells` (31 cases) and `--suite bodies` (32 cases) on the RTX 5070 Ti, NVIDIA 615.78.08, at 04. The `streaming` and `render` suites belong to 05–06 and fail until registered. |
| Connected scenario | Partial (04) | `cargo evidence run --scenario resident-world --adapter vulkan-10de-2c05`: known terrain, the sphere across sand into water, a GPU-ray edit and the CPU consumer pass; focus, reload and checkpoint fail as not implemented (05, 07). |
| Primary hardware performance | Measured baseline (01); GPU physics measured (03, 04) | Legacy application: summaries in `tools/evidence/baselines/legacy-6e108d6/`. The GPU material phases' device times at radius 6 ([budgets](../reference/performance-budgets.md#material-phases-03)) and the bodies at 64–4,096 bodies, on the legacy body workloads and paced at 60 Hz ([budgets](../reference/performance-budgets.md#bodies-04)) are measured headlessly; physics only, not a native frame time. |
| Native interactive scenario | Not run | Current and target applications both unqualified. |
| Fault recovery and legacy save import | Not run | The legacy codec is fixture-tested; import and recovery are 05. |
| Secondary GPU / Windows / other platforms | Unqualified | No compatibility claim. Hosted CI runs the CPU and static lanes on Linux only. |

After each completed stage, replace affected rows with the tested commit, command, environment and
result. Keep detailed logs in artifacts, not this page. At sprint removal, replace the temporary
source-audit link with the retained evidence location or a compact permanent baseline summary.

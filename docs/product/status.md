# Current status

**Snapshot:** prompt 03, 2026-10-17, branch `experimental`. The baseline application still owns the
world on the CPU; 01 measured it, pinned its build, extracted the framework-free model it now
uses and added the evidence runner. 02 added the GPU boundary: a runtime and a GPU owner that
execute ordered brush edits, column queries and chunk transfers on the device. 03 runs every
material rule family on the GPU owner, with exact accounting and sparse tile scheduling, tested
headlessly against an independent CPU reference — on a software adapter only, because the
nominated GPU's driver did not load during 03. Bodies are not on the GPU yet, and the baseline
application does not use the new owner.
Update this page when implementation produces evidence; do not convert target architecture into
past-tense claims.

## Current implementation

| Area | Observed implementation | Qualification |
| --- | --- | --- |
| Workspace | `crates/{monarch-engine,morbid-app,spatial-lib,world-model,world-runtime}`, `adapters/gpu-world` and `tools/evidence`. Rust 1.99.0 is pinned in `rust-toolchain.toml`; `Cargo.lock` is tracked (Bevy 0.19.0, `bevy_egui` 0.41.0, egui 0.35.0, wgpu 29.0.4, winit 0.30.13, redb 4.1.0, bitcode 0.6.9). | Builds with `--locked`; `check static` passes, with 3 recorded Clippy exceptions (7 diagnostics in `morbid-app`, removed in 06). Six locked crates were updated past RustSec advisories after the baseline was measured; four advisories without a compatible fix are accepted in `deny.toml` with reasons. |
| World model | `world-model` owns packed cells and their column rules, materials, the automaton's rules (`rules`: switches, cadences, tuning constants, wake levels, keyed hashes and shuffles), chunk keys, units, supported settings, body presets, commands (including the column query, configuration and totals), observations and generation. Engine, application, runtime, GPU owner and runner import it. Since 03 the quench converts only what fits under the elevation ceiling. | 31 tests, including hand-written layout goldens, the baseline hash written out by hand and independent ceiling fixtures; legacy fixtures decode and re-encode byte for byte. |
| Runtime | `world-runtime`: session, command and tick identities; the `PhysicalWorld` port with per-tick material accounting; bounded receipts (retry applies once, reuse conflicts, acknowledged identities expire); the outcome ledger separating important results from counted cosmetic drops and counting unbalanced ticks; the fixed, reconfigurable clock; tick-scheduled commands; a standalone `bevy_ecs` runtime. Used headlessly by the GPU suites; the baseline does not use it. | 24 tests of its rules; exercised with the GPU owner in the `transfers` and `cells` suites. |
| GPU world | `gpu-world`: explicit adapter selection, requested limits, one submission timeline (≤ 3 in flight), checked resource plans up to radius 32, generated layouts, material tables and rule constants, assembled WGSL validated offline, chunk slots, ordered brush edits, column queries, configuration and totals commands, pooled upload/readback staging, resize with device-to-device migration, loss, fault and stop handling. Since 03 every tick runs the material phases (propose, resolve, react, commit) over the tiles its activity scheduling selects on the device, sparse or dense. No body simulation. | 27 tests. `transfers` (28 cases) passed on the RTX 5070 Ti at 02; at 03 it ran on llvmpipe with only its two known software-adapter failures. `cells` (31 cases) passed on llvmpipe; not run on hardware. [Measured resources](../reference/performance-budgets.md#material-phases-03). |
| Terrain | Baseline: eight-byte packed columns, double-buffered CPU automaton, wake/halo tracking, Rayon traversal; since 03 it reads its constants and hashes from `world_model::rules`, with identical results. GPU (03): the same rules over the same cells as WGSL kernels, every transfer proposed once and accepted against the receiver's capacity; idle tiles are neither visited nor copied. | 10 baseline simulation tests pass. The CPU reference in `monarchy-evidence` equals the baseline engine cell for cell on its fixtures, and the GPU equals the reference tick for tick in the `cells` suite (llvmpipe). Measured: the baseline still copies the whole array every tick; the GPU path visits no tile in an idle world. |
| Bodies | CPU sphere integration, terrain contact/deformation, fluid displacement, all-pairs sphere checks. | 12 body tests pass. Measured: up to 261,120 pair tests per frame at 256 bodies. |
| Material contracts | Column mixing, phase changes and capacity helpers in `world-model`; the per-operation conservation, sinks and capacity policy in the [GPU contract](../reference/gpu-contract.md#material-accounting-03). | 5 baseline cell tests, 3 ceiling tests and 1 apportionment test pass; the WGSL rules equal them on 15,488 boundary columns on the device. Every GPU tick reports exact 64-bit accounting that the ledger checks. |
| Rendering | Bevy material shaders. Four layer entities per active chunk (676 at radius 6, counted from the measured active chunks, not from draw calls) share one cell buffer; a dirty grid uploads the whole array. | Measured GPU passes and frame times (see the baseline below); visual reference frames and the lighting/quality defaults are recorded for 06. |
| Streaming/storage | Toroidal CPU grid. Departing chunks wait in an LRU cache and are saved only when evicted, with no dirty tracking. redb operations run asynchronously; `legacy_format` owns the row bytes. | Fixture-tested codec. Failure/restart safety not qualified. |
| Interaction | Brushes, metal/wood sphere spawning, possession, attract/lift, observer walk/noclip, camera, and simulation/lighting/resize UI. UI ranges come from `world_model::config`. | Exercised through scripted scenarios; no interactive session was qualified. |
| Event path | In the baseline, event types and an unbounded queue exist; nothing consumes them. The GPU path has the bounded replacement: command outcomes and residency results with reserved storage, and a per-tick cosmetic event buffer whose drops are counted, consumed by `world-runtime`'s ledger. Its only producer is fault injection until 04 adds contacts. | The consumer and overflow accounting are tested on the GPU; not yet a gameplay notification system. |

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
Terrain advances on a fixed clock; bodies subdivide frame time separately, and their
registration defines no terrain/body order. Save tasks log failed writes without retaining their
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
finished (same size, same modification time); neither opened it. On startup the baseline opens or
creates it relative to the working directory. Experiments, fixtures and benchmarks use temporary
stores and a scratch working directory. See the
[development guide](../guides/development.md#baseline-versus-target).

## Target work not yet implemented

Bodies on the GPU (04), the residency policy and checkpoint protocol (05), and the native renderer
and client (06) are pending, as is qualification of the GPU material phases on the nominated
hardware (blocked during 03 by the driver mismatch; rerun the `cells` suite on
`vulkan-10de-2c05`). The GPU owner, its transfers and shader validation exist since 02; the GPU
material phases, their accounting and sparse scheduling since 03.

## Evidence ledger

| Lane | State | Evidence |
| --- | --- | --- |
| Documentation | Passed (01) | `cargo evidence check docs`: links and anchors, owners, sprint structure, retired rules, package READMEs and their naming in the architecture. |
| Rust compilation, linting and dependency policy | Passed (02) | `cargo evidence check static`: rustfmt, three feature combinations, Clippy against 3 recorded exceptions (7 diagnostics, removal in 06), rustdoc with warnings denied, naga validation of the assembled shaders, `cargo deny`, `cargo machete`, dependency-direction rules per package, pins and CI. |
| CPU tests | Passed (02) | `cargo evidence test cpu`: every test of the production owners, checked against 96 registered behaviors (28 baseline). |
| GPU correctness and buffer layout | `transfers` passed on hardware (02); `cells` passed on a software adapter only (03) | `cargo evidence test gpu --suite transfers --adapter vulkan-10de-2c05`: 28 cases on the RTX 5070 Ti at 02. `cargo evidence test gpu --suite cells --adapter gl-10005-0000`: 31 cases on Mesa llvmpipe at 03 (the RTX 5070 Ti's driver did not load); hardware unqualified. The `bodies`, `streaming` and `render` suites belong to 04–06 and fail until registered. |
| Primary hardware performance | Measured baseline (01) | Legacy application only; summaries in `tools/evidence/baselines/legacy-6e108d6/`. The capacity profile is frozen but not run. The GPU material phases' activity measurement ran only on llvmpipe (structural results, no hardware timing). |
| Native interactive scenario | Not run | Current and target applications both unqualified. |
| Fault recovery and legacy save import | Not run | The legacy codec is fixture-tested; import and recovery are 05. |
| Secondary GPU / Windows / other platforms | Unqualified | No compatibility claim. Hosted CI runs the CPU and static lanes on Linux only. |

After each completed stage, replace affected rows with the tested commit, command, environment and
result. Keep detailed logs in artifacts, not this page. At sprint removal, replace the temporary
source-audit link with the retained evidence location or a compact permanent baseline summary.

# Current status

**Snapshot:** prompt 06, 2026-10-20, branch `experimental`. The native client (`morbid-app`,
`apps/morbid`) is the application: a winit window with an egui overlay over the runtime's
`WorldSession`, the saved world in `world-store`, the GPU owner `gpu-world` and the renderer
`world-render`, all on one device. The GPU owns cells and bodies (03–04); the CPU issues intent,
manages residency and saves (05). 06 removed the Bevy baseline, its CPU engine (`monarch-engine`)
and its chunk manager (`spatial-lib`); `9a3230c` is the last commit that builds them. 07 qualifies
the whole. Update this page when implementation produces evidence; do not convert target
architecture into past-tense claims.

## Current implementation

| Area | Observed implementation | Qualification |
| --- | --- | --- |
| Workspace | `apps/morbid` (`morbid-app`), `crates/{world-model,world-render,world-runtime}`, `adapters/{gpu-world,world-store}` and `tools/evidence`. Rust 1.99.0 is pinned in `rust-toolchain.toml`; `Cargo.lock` is tracked (wgpu 29.0.4, winit 0.30.13, egui, egui-wgpu and egui-winit 0.35.0, standalone `bevy_ecs` 0.19.0, redb 4.1.0, bitcode 0.6.9). The native client's resolved closure has no Bevy engine crate. | Builds with `--locked`; `check static` passes with an empty lint baseline. One advisory without a compatible fix (quick-xml, build-time only, through wayland-scanner) is accepted in `deny.toml` with its reason. |
| World model | `world-model` owns packed cells and their column rules, materials, the automaton's rules (`rules`), chunk keys, units, supported settings, body presets and tuning, commands (rays, targets, queries, configuration, totals), observations, the layout and physics fingerprints (`identity`), body resume records (`resume`) and generation. The runtime, GPU owner (through generated declarations), renderer, client and runner import it. | 41 tests, including hand-written layout goldens, the baseline hash written out by hand, independent ceiling fixtures, the substep and apportionment rules and the pinned fingerprints. |
| Runtime | `world-runtime`: identities, the `PhysicalWorld` port with per-tick accounting, bounded receipts, the outcome ledger, the body monitor, the fixed reconfigurable clock, tick-scheduled commands, the storage port and workers, the residency and checkpoint owners and `WorldSession` (open, save, autosave, orderly close, loss report), in a standalone `bevy_ecs` world. Its consumers are the native client, the GPU suites, `resident-world` and `cargo evidence session`. | 44 tests, including the residency, checkpoint and session owners against a model port and an in-memory store; exercised with the GPU owner in every GPU suite and the scenario. |
| GPU world | `gpu-world`: explicit adapter selection, requested limits, one submission timeline (≤ 3 world submissions in flight), checked resource plans up to radius 32, generated layouts, assembled WGSL validated offline, chunk slots, the material phases with sparse tile scheduling (03), bodies, rays and every body command (04), change words, chunk and world snapshots and resize (05). Since 06 it opens a session presenting to a window (`open_presenting`, the adapter's backend only) and takes the renderer's frames on the same timeline with their own bound of two (`submit_presentation`). | 34 tests. `transfers` (28 cases), `cells` (31), `bodies` (32) and `streaming` (19) pass on the RTX 5070 Ti at `da1c3ba`. Measured: an idle radius-6 window costs 27 µs of device time per tick, a fully active one 280 µs ([material phases](../reference/performance-budgets.md#material-phases-03)); the legacy qualification body workload's body pass takes 0.63 ms ([bodies](../reference/performance-budgets.md#bodies-04)). |
| Terrain | GPU only: the column rules as WGSL kernels over the device cells, every transfer proposed once and accepted against the receiver's capacity; idle tiles are neither visited nor copied. The CPU automaton was removed in 06; its ten simulation tests run as `cells` cases. | The CPU reference in `monarchy-evidence` matched the baseline engine cell for cell on its fixtures until the engine was removed, and the GPU equals the reference tick for tick in the `cells` suite. |
| Bodies | GPU only: fixed substeps of the tick, hashed bins, swept contacts, a speed cap of 600 units/s, shared deformation and exact material accounting. The CPU sphere integration was removed in 06; its twelve tests and the apportionment test run as `bodies` cases. | Measured on the frozen legacy workloads: the baseline's body time per tick was 5.32 ms (CPU, 64 spheres) and 16.99 ms (256); the GPU owner takes 0.63 ms and 0.98 ms of device time. |
| Material contracts | Column mixing, phase changes and capacity helpers in `world-model`; the per-operation conservation, sinks and capacity policy in the [GPU contract](../reference/gpu-contract.md#material-accounting-03), and the bodies' accounting ([bodies](../reference/gpu-contract.md#bodies-04)). | 5 baseline cell tests and 3 ceiling tests pass on the CPU; the WGSL rules equal them on 15,488 boundary columns on the device. Every GPU tick reports exact 64-bit accounting of cells and bodies that the ledger checks. |
| Rendering | `world-render` (06): the four landscape layers drawn per window chunk from the device cells (the baseline's landscape material, ported), spheres instanced from the device body state, GPU visibility (chunk bounds, frustum culling and ordered compaction into indirect draws), a camera that follows a body from its device pose, heightfield shadows and occlusion, a sphere shadow map, fog with sun glow, bloom and AgX tone mapping. It writes no physical state and keeps no copy of it. | 12 tests; the `render` suite's 12 cases pass offscreen on the RTX 5070 Ti at `da1c3ba`. Measured at 1920×1080 from the qualification camera: scene passes 0.84 ms at radius 6 and 4.20 ms at radius 16 (1.81 ms and 11.75 ms unculled), post 0.09–0.10 ms ([rendering](../reference/performance-budgets.md#native-client-06)). |
| Streaming/storage | `world-store` format version 1 (CRC-32C rows, immutable chunk and body revisions, atomic publication, stale-save refusal, reclaim after an unclean session), the legacy import, and the runtime's residency, checkpoint and session owners over the GPU owner; the client creates, opens, imports, saves, autosaves and closes worlds through them; the `world-store` command and `cargo evidence session` remain for scripts. | The `streaming` suite (19 cases) with real temporary stores, injected I/O errors and process interruption; `world-store`'s 36 tests of the format, the store, imports and the command against real files and the old writer's fixtures. Measured: the ordinary resident simulation moves no world data, a save reads back only changed chunks ([streaming and checkpoints](../reference/performance-budgets.md#streaming-and-checkpoints-05)). |
| Native client | `morbid-app` (06): a pumped winit loop, an egui overlay (tool bar, simulation, lighting, statistics, world, outcome and pressure panels, compass), the world catalog and settings outside the working directory, explicit adapter choice, open/close/save feedback, device-loss report and recovery under a later epoch, every inventory tool as runtime commands at GPU-resolved rays and column queries. | 8 tests; in its real window on `da1c3ba` the `features` script passed 41 checks (every inventory group through the input path), `loss` 6 (save, loss, recovery from the latest checkpoint, the report on screen) and `worlds` 4 (create, import, close with save, reopen). |
| Interaction | Every brush, sphere preset, drive, jump, attract and lift as commands at a column or a GPU-resolved ray, through the runtime's receipts; walking on column queries; statistics from GPU totals. | The `features` script checks each tool against its command outcome in the real window; GPU commands, rays and retries are also tested headlessly. |
| Event path | Command outcomes and residency results with reserved storage, a per-tick cosmetic event buffer whose drops are counted, body samples and per-body counters consumed by the runtime's body monitor, whose events are deferred, never dropped, under pressure; the client logs them. The baseline's unconsumed event queue went with it. | Delivery under a saturated ring, delayed and reordered samples and slot reuse is tested on the GPU (`observation-delivery`), and `resident-world`'s consumer reacts to it; not yet a gameplay notification system. |

The sprint's [source audit](../development/virtual-office/gpu-transition/source-audit.md) records
the baseline's symbols and what 01 found while measuring.

## Measured baseline and native frame

The Bevy baseline, measured at `6e108d6` on the nominated machine (Ryzen 9 5950X, RTX 5070 Ti,
Vulkan, NVIDIA 615.71.09, KDE Wayland), five valid runs per case: the qualification profile
(radius 6, 64 bodies, 1920×1080) missed its p95 and p99 targets in body-contact (p95 18.7 ms) and
coupled (p95 28.6 ms, p99 32.1 ms) and met them in idle, flow, mixed and streaming; the scale
profile fell behind real time in every active case (17–30 ticks/s). The tables are in
[performance budgets](../reference/performance-budgets.md#measured-legacy-baseline-01).

The native client, measured on 2026-10-20 in its window on the same machine (NVIDIA 615.78.08,
Linux 7.2.11), one valid run per qualification case (prequalification, not the five-run
protocol): frame medians 1.06–1.23 ms, p95 10.63–10.74 ms, p99 10.85–10.99 ms, no frame over
16.67 ms, 60 ticks/s, about 0.46 ms of CPU and 0.2–0.9 ms of device time per tick. Every case
meets the qualification budget in that run; the p95 in idle and streaming is above the
baseline's because of waits in swapchain image acquisition. The table and conditions are in
[performance budgets](../reference/performance-budgets.md#native-client-06).

## Known gaps

- `resident-world` step 6 runs 05's faults and recovery and fails until 07 adds delayed
  consumers, dense body work at chunk seams and the integrated run.
- On KWin Wayland with the NVIDIA driver about 15% of presented frames wait a constant 10.09 ms to acquire a swapchain image, which sets the native frame's p95 near 10.7 ms; the source (compositor or driver) is not yet known.
- Saving and streaming add about 0.4 ms at p95 and 190–350 allocations to a frame (05), not yet
  broken down by source.
- No command journal: changes after the last checkpoint are lost on a crash or device loss, and
  the loss report says how many ticks. A world whose layout, physics or generator identity
  differs is refused, not converted.
- Presentation is qualified on Vulkan on Linux (KWin Wayland) only.

## User data

`world_data/save.redb` (git-ignored) at the repository root is a save the baseline generated; the
user said on 2026-10-19 that it holds nothing important. The native client never opens it: its
worlds live in the data directory, and the start screen can import a new world from a legacy
save. Experiments, fixtures, scripts and benchmarks use temporary stores, for reproducibility. See
the [development guide](../guides/development.md#the-application).

## Target work not yet implemented

07: the integrated qualification (repeated benchmark runs of every profile, the comparison with
the baseline's measurements, `resident-world` step 6 and the whole trace in the native client,
adversarial load) and the sprint's closure.

## Evidence ledger

| Lane | State | Evidence |
| --- | --- | --- |
| Documentation | Passed (06) | `cargo evidence check docs`: links and anchors, owners, sprint structure, retired rules, package READMEs and their naming in the architecture. |
| Rust compilation, linting and dependency policy | Passed (06) | `cargo evidence check static`: rustfmt, two feature combinations, Clippy with no exceptions, rustdoc with warnings denied, naga validation of the assembled `gpu-world` and `world-render` shaders, `cargo deny`, `cargo machete`, dependency-direction rules per package (the native client's closure has no Bevy engine crate), pins and CI. |
| CPU tests | Passed (06) | `cargo evidence test cpu`: 232 tests passed, checked against the registry's 177 CPU behaviors and 57 runner tests; the 28 baseline behaviors stay registered, 23 of them as GPU cases since 06. |
| GPU correctness and buffer layout | Passed (06, every suite) | `cargo evidence test gpu --suite transfers --adapter vulkan-10de-2c05` (28 cases), `--suite cells` (31), `--suite bodies` (32), `--suite streaming` (19) and `--suite render` (12) on the RTX 5070 Ti, NVIDIA 615.78.08, Linux 7.2.11, at `da1c3ba`. |
| Connected scenario | Partial (05; visible since 06) | `cargo evidence run --scenario resident-world --adapter vulkan-10de-2c05`, headless and with `--consumer native` in the client's window: steps 1–5 pass; the adversarial repeat runs 05's faults and recovery and fails until 07. |
| Native interactive client | Passed (06, scripted in the real window) | `native-client script features` (41 checks), `loss` (6) and `worlds` (4) in the real window on `vulkan-10de-2c05` at `da1c3ba`; `resident-world --consumer native` (steps 1–5); a frame captured at the idle reference camera compared with the baseline's ([acceptance](../development/virtual-office/gpu-transition/acceptance.md#baseline-presentation-and-tuning-reference-01)). |
| Primary hardware performance | Measured baseline (01); GPU physics (03–05); native frame (06, prequalification) | Legacy application: summaries in `tools/evidence/baselines/legacy-6e108d6/`. GPU material phases, bodies, streaming and the renderer's passes in [budgets](../reference/performance-budgets.md). The native frame: `cargo evidence bench --profile qualification --adapter vulkan-10de-2c05 --runs 1` at `5bf7b44` and, for the three cases whose windows the desktop shrank there, at `da1c3ba`; every case within the qualification budget in one run ([budgets](../reference/performance-budgets.md#native-client-06)). |
| Fault recovery and legacy save import | Passed (05 headless; 06 in the client) | `streaming` cases for failed and retried commits, retention pressure, delayed mapping, orderly close and timeout, device loss before and after capture, and process interruption; `world-store` tests for interruption and import; the client's `loss` and `worlds` scripts. |
| Secondary GPU / Windows / other platforms | Unqualified | No compatibility claim. Hosted CI runs the CPU and static lanes on Linux only. |

After each completed stage, replace affected rows with the tested commit, command, environment and
result. Keep detailed logs in artifacts, not this page. At sprint removal, replace the temporary
source-audit link with the retained evidence location or a compact permanent baseline summary.

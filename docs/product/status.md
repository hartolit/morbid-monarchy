# Current status

**Snapshot:** prompt 07, 2026-10-21, branch `experimental`, code at `eb9d8fb`. The GPU transition is
complete and qualified on the nominated desktop ([qualification](../development/qualification.md)).
The native client (`morbid-app`, `apps/morbid`) is the application: a winit window with an egui
overlay over the runtime's `WorldSession`, the saved world in `world-store`, the GPU owner
`gpu-world` and the renderer `world-render`, all on one device. The GPU owns cells and bodies;
the CPU issues intent, manages residency and saves. 06 removed the Bevy baseline, its CPU engine
(`monarch-engine`) and its chunk manager (`spatial-lib`); `9a3230c` is the last commit that builds
them. Update this page when implementation produces evidence; do not convert target architecture
into past-tense claims.

## Current implementation

| Area | Observed implementation | Qualification |
| --- | --- | --- |
| Workspace | `apps/morbid` (`morbid-app`), `crates/{world-model,world-render,world-runtime}`, `adapters/{gpu-world,world-store}` and `tools/evidence`. Rust 1.99.0 is pinned in `rust-toolchain.toml`; `Cargo.lock` is tracked (wgpu 29.0.4, winit 0.30.13, egui, egui-wgpu and egui-winit 0.35.0, standalone `bevy_ecs` 0.19.0, redb 4.1.0, bitcode 0.6.9). The native client's resolved closure has no Bevy engine crate. | Builds with `--locked`; `check static` passes with an empty lint baseline. One advisory without a compatible fix (quick-xml, build-time only, through wayland-scanner) is accepted in `deny.toml` with its reason. |
| World model | `world-model` owns packed cells and their column rules, materials, the automaton's rules (`rules`), chunk keys, units, supported settings, body presets and tuning, commands (rays, targets, queries, configuration, totals), observations, the layout and physics fingerprints (`identity`), body resume records (`resume`) and generation. The runtime, GPU owner (through generated declarations), renderer, client and runner import it. | 41 tests, including hand-written layout goldens, the baseline hash written out by hand, independent ceiling fixtures, the substep and apportionment rules and the pinned fingerprints. |
| Runtime | `world-runtime`: identities, the `PhysicalWorld` port with per-tick accounting, bounded receipts, the outcome ledger, the body monitor, the fixed reconfigurable clock, tick-scheduled commands, the storage port and workers, the residency and checkpoint owners and `WorldSession` (open, save, autosave, orderly close, loss report), in a standalone `bevy_ecs` world. Its consumers are the native client, the GPU suites, `resident-world` and `cargo evidence session`. | 44 tests, including the residency, checkpoint and session owners against a model port and an in-memory store; exercised with the GPU owner in every GPU suite and the scenario. |
| GPU world | `gpu-world`: explicit adapter selection, requested limits, one submission timeline (≤ 3 world submissions in flight), checked resource plans up to radius 32, generated layouts, assembled WGSL validated offline, chunk slots, the material phases with sparse tile scheduling (03), bodies, rays and every body command (04), change words, chunk and world snapshots and resize (05). Since 06 it opens a session presenting to a window (`open_presenting`, the adapter's backend only) and takes the renderer's frames on the same timeline with their own bound of two (`submit_presentation`). | 34 tests. `transfers` (28 cases), `cells` (31), `bodies` (32) and `streaming` (19) pass on the RTX 5070 Ti at `eb9d8fb` (07). Measured: an idle radius-6 window costs 27 µs of device time per tick, a fully active one 280 µs ([material phases](../reference/performance-budgets.md#material-phases-03)); the legacy qualification body workload's body pass takes 0.63 ms ([bodies](../reference/performance-budgets.md#bodies-04)). |
| Terrain | GPU only: the column rules as WGSL kernels over the device cells, every transfer proposed once and accepted against the receiver's capacity; idle tiles are neither visited nor copied. The CPU automaton was removed in 06; its ten simulation tests run as `cells` cases. | The CPU reference in `monarchy-evidence` matched the baseline engine cell for cell on its fixtures until the engine was removed, and the GPU equals the reference tick for tick in the `cells` suite. |
| Bodies | GPU only: fixed substeps of the tick, hashed bins, swept contacts, a speed cap of 600 units/s, shared deformation and exact material accounting. The CPU sphere integration was removed in 06; its twelve tests and the apportionment test run as `bodies` cases. | Measured on the frozen legacy workloads: the baseline's body time per tick was 5.32 ms (CPU, 64 spheres) and 16.99 ms (256); the GPU owner takes 0.63 ms and 0.98 ms of device time. |
| Material contracts | Column mixing, phase changes and capacity helpers in `world-model`; the per-operation conservation, sinks and capacity policy in the [GPU contract](../reference/gpu-contract.md#material-accounting-03), and the bodies' accounting ([bodies](../reference/gpu-contract.md#bodies-04)). | 5 baseline cell tests and 3 ceiling tests pass on the CPU; the WGSL rules equal them on 15,488 boundary columns on the device. Every GPU tick reports exact 64-bit accounting of cells and bodies that the ledger checks. |
| Rendering | `world-render` (06): the four landscape layers drawn per window chunk from the device cells (the baseline's landscape material, ported), spheres instanced from the device body state, GPU visibility (chunk bounds, frustum culling and ordered compaction into indirect draws), a camera that follows a body from its device pose, heightfield shadows and occlusion, a sphere shadow map, fog with sun glow, bloom and AgX tone mapping. It writes no physical state and keeps no copy of it. | 12 tests; the `render` suite's 12 cases pass offscreen on the RTX 5070 Ti at `eb9d8fb` (07). Measured at 1920×1080 from the qualification camera: scene passes 0.84 ms at radius 6 and 4.20 ms at radius 16 (1.81 ms and 11.75 ms unculled), post 0.09–0.10 ms ([rendering](../reference/performance-budgets.md#native-client-06)). |
| Streaming/storage | `world-store` format version 1 (CRC-32C rows, immutable chunk and body revisions, atomic publication, stale-save refusal, reclaim after an unclean session), the legacy import, and the runtime's residency, checkpoint and session owners over the GPU owner; the client creates, opens, imports, saves, autosaves and closes worlds through them; the `world-store` command and `cargo evidence session` remain for scripts. | The `streaming` suite (19 cases) with real temporary stores, injected I/O errors and process interruption; `world-store`'s 36 tests of the format, the store, imports and the command against real files and the old writer's fixtures. Measured: the ordinary resident simulation moves no world data, a save reads back only changed chunks ([streaming and checkpoints](../reference/performance-budgets.md#streaming-and-checkpoints-05)). |
| Native client | `morbid-app` (06): a pumped winit loop, an egui overlay (tool bar, simulation, lighting, statistics, world, outcome and pressure panels, compass), the world catalog and settings outside the working directory, explicit adapter choice, open/close/save feedback, device-loss report and recovery under a later epoch, every inventory tool as runtime commands at GPU-resolved rays and column queries. | 8 tests; in its real window on `eb9d8fb`'s tree (07) the `features` script passed 41 checks in each of three runs (every inventory group through the input path), `loss` 6 (save, loss, recovery from the latest checkpoint, the report on screen) and `worlds` 5 (create, import, refusal of a damaged save, close with save, reopen); the copied executable started fresh outside the repository with an empty home directory, opened a world and closed it through the compositor with its save ([qualification](../development/qualification.md#packaged-fresh-start)). |
| Interaction | Every brush, sphere preset, drive, jump, attract and lift as commands at a column or a GPU-resolved ray, through the runtime's receipts; walking on column queries; statistics from GPU totals. | The `features` script checks each tool against its command outcome in the real window; GPU commands, rays and retries are also tested headlessly. |
| Event path | Command outcomes and residency results with reserved storage, a per-tick cosmetic event buffer whose drops are counted, body samples and per-body counters consumed by the runtime's body monitor, whose events are deferred, never dropped, under pressure; the client logs them. The baseline's unconsumed event queue went with it. | Delivery under a saturated ring, delayed and reordered samples and slot reuse is tested on the GPU (`observation-delivery`); `resident-world`'s consumer reacts to it, and its step 6 drains the events of 62 piled spheres after a long absence, every one adding up to the bodies' counters; not yet a gameplay notification system. |

The [qualification](../development/qualification.md#baseline-dispositions) records what the
baseline did and where each of its responsibilities went.

## Measured result

The Bevy baseline, measured at `6e108d6` on the nominated machine (Ryzen 9 5950X, RTX 5070 Ti,
Vulkan, KDE Wayland), five valid runs per case, missed the qualification budget in body-contact
(p95 18.7 ms) and coupled (p95 28.6 ms, p99 32.1 ms) and fell behind real time in every active
scale case (17–30 ticks/s). The native client, measured on 2026-10-21 at `eb9d8fb` with the same
protocol (five valid runs per case of every profile, a clean tree, a quiet host):

| Profile | Frame median native / baseline | p95 native / baseline | Worst p99 | Ticks/s native / baseline | CPU per tick (simulation) | Grid transfer |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Qualification (radius 6, 64 bodies) | 1.07–1.24 / 3.57–17.12 ms | 1.3–10.8 / 6.1–28.6 ms | 10.98 ms | 60 / 60 | 0.07–0.28 / 2.1–13.1 ms | none / 0.60–0.85 GiB/s |
| Scale (radius 16, 256 bodies) | 4.55–4.90 / 27.0–223.8 ms | 5.6–10.8 / 36.7–252.6 ms | 11.01 ms | 60 / 17–60 | 0.30 with bodies / 7.6–51.6 ms | none / 0.76–3.15 GiB/s |
| Capacity (radius 32, up to 4,096 bodies) | 18.0–27.1 ms | 21–49 ms | 52.1 ms | 51–60 | – | none |

Every qualification run meets the budget (p95 ≤ 16.67 ms, p99 ≤ 25 ms); the scale profile does
too, with no frame over 16.67 ms; capacity admits its workloads and reports when it falls behind
(counted discarded ticks, pressure) instead of truncating physics. The native p95 is set by
10.09 ms waits for a swapchain image when presentation runs faster than the display (without
vsync, as the frozen profiles present); they come and go between runs and do not occur with
vsync. Simulation CPU per tick is the encoding and submission of a tick and the session's owners,
measured by the GPU suites on the same commit. The tables, conditions, transfers, memory,
allocations and the capacity workload's limits are in the
[performance budgets](../reference/performance-budgets.md#integrated-qualification-07).

## Known gaps

- Without vsync, the KWin Wayland and Xwayland paths with the NVIDIA driver make up to 22% of
  presented frames wait a constant 10.09 ms for a swapchain image (none in some runs), which
  sets the p95 near 10.7 ms in the runs where it happens. The wait does not depend on the swapchain's length and does not occur
  with vsync; attributing it to the driver is an inference.
- Without vsync the client's loop never blocks: it runs about 4 iterations per presented frame at
  radius 6 and about 20 at radius 16, each laying out the overlay (0.9–5.4 ms of main-thread CPU
  and 2,000–10,800 allocations per presented frame). With vsync it runs one per refresh.
- With 4,096 dense bodies at radius 32 a tick takes about 8 ms of device time and the simulation
  falls to 51–53 ticks/s, discarding owed ticks visibly; the capacity profile promises no rate.
- The benchmark's pacing rule can reject runs whose saturated frames end at the 60 Hz tick
  cadence (capacity coupled, three of seven attempts in one invocation), although no compositor
  throttles them.
- No command journal: changes after the last checkpoint are lost on a crash or device loss, and
  the loss report says how many ticks. A world whose layout, physics or generator identity
  differs is refused, not converted.
- Presentation and performance are qualified on Vulkan on Linux (KWin Wayland, NVIDIA 615.78.08,
  RTX 5070 Ti) only.

## User data

`world_data/save.redb` (git-ignored) at the repository root is a save the baseline generated; the
user said on 2026-10-19 that it holds nothing important. The native client never opens it: its
worlds live in the data directory, and the start screen can import a new world from a legacy
save. Experiments, fixtures, scripts and benchmarks use temporary stores, for reproducibility. See
the [development guide](../guides/development.md#the-application).

## Target work not yet implemented

None in the GPU transition, which is complete. What to do next is chosen from the measured
constraints above ([roadmap](roadmap.md#after-qualification)).

## Evidence ledger

| Lane | State | Evidence |
| --- | --- | --- |
| Documentation | Passed (07) | `cargo evidence check docs`: links and anchors, owners, assignment structure, retired rules, package READMEs and their naming in the architecture; passed on the closing tree after the sprint's removal. |
| Rust compilation, linting and dependency policy | Passed (07) | `cargo evidence check static` on `eb9d8fb`'s tree: rustfmt, two feature combinations, Clippy with no exceptions, rustdoc with warnings denied, naga validation of the assembled `gpu-world` and `world-render` shaders, `cargo deny`, `cargo machete`, dependency-direction rules per package (only the standalone ECS crates in any closure), pins and CI. |
| CPU tests | Passed (07) | `cargo evidence test cpu` on `eb9d8fb`'s tree: 232 tests, checked against the registry's 177 CPU behaviors and 57 runner tests; the 28 baseline behaviors stay registered, 23 of them as GPU cases. |
| GPU correctness and buffer layout | Passed (07, every suite) | `cargo evidence check all --adapter vulkan-10de-2c05` on `eb9d8fb`'s tree (152 of 152 items): `transfers` (28 cases), `cells` (31), `bodies` (32), `streaming` (19), `render` (12) on the RTX 5070 Ti, NVIDIA 615.78.08, Linux 7.2.11. |
| Connected scenario | Passed (07, both consumers) | `cargo evidence run --scenario resident-world --adapter vulkan-10de-2c05`, headless (in `check all`) and with `--consumer native`: all six steps, step 6 the adversarial repeat. |
| Native interactive client | Passed (07, scripted in the real window) | `native-client script features` (41 checks, three runs), `loss` (6) and `worlds` (5) on `eb9d8fb`'s tree; the packaged fresh start outside the repository. |
| Primary hardware performance | Qualified (07) | `cargo evidence bench --profile qualification`, `scale` and `capacity` with `--runs 5` at `eb9d8fb`, compared with the baseline's summaries in `tools/evidence/baselines/legacy-6e108d6/` ([budgets](../reference/performance-budgets.md#integrated-qualification-07)). |
| Fault recovery and legacy save import | Passed (07) | `streaming` cases; `resident-world` step 6 (failed writes, a failed and retried save, device loss, process interruption before and after publication, orderly close, fresh reopen); `world-store` tests; the `loss` and `worlds` scripts; a damaged legacy save refused by the start screen and by `world-store import-legacy`. |
| Secondary GPU / Windows / other platforms | Unqualified | No compatibility claim. Hosted CI runs the CPU and static lanes on Linux only. |

Replace affected rows with the tested commit, command, environment and result when later work
produces evidence. Keep detailed logs in artifacts, not this page.

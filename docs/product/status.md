# Current status

**Snapshot:** 2026-10-22, branch `experimental`, code at `67edbc2`. The GPU transition is complete
and qualified on the nominated desktop at `eb9d8fb`
([qualification](../development/qualification.md)); since then the benchmark's pacing rule counts
only consecutive frames (`c4271ad`), the client's loop waits instead of spinning without vsync
(`2c63b64`), measured again with 07's protocol, and a person's world is checkpointed every 60 ticks
by default and never less often than every 3,600, which bounds what a crash or a lost device loses
(`2932764`, [ADR 0004](../decisions/0004-bounded-loss-by-checkpoint-cadence.md)), and at radius 32 a
world at rest no longer keeps dry foliage awake (`10cf62f`, physics revision 2) and the landscape's
lighting is cached per chunk vertex and relit only where cells changed (`63f8ce6`, `d4d2d6c`), and a
window move keeps the window's buffers and wakes nothing beyond its rows (`67edbc2`), all measured
again in the window with 07's protocol on `6e3f2d9`. The native client (`morbid-app`, `apps/morbid`)
is the application: a winit window with an egui overlay over the runtime's `WorldSession`, the saved
world in `world-store`, the GPU owner `gpu-world` and the renderer `world-render`, all on one
device. The GPU owns cells and bodies; the CPU issues intent, manages residency and saves. 06
removed the Bevy baseline, its CPU engine (`monarch-engine`) and its chunk manager (`spatial-lib`);
`9a3230c` is the last commit that builds them. Update this page when implementation produces
evidence; do not convert target architecture into past-tense claims.

## Current implementation

| Area | Observed implementation | Qualification |
| --- | --- | --- |
| Workspace | `apps/morbid` (`morbid-app`), `crates/{world-model,world-render,world-runtime}`, `adapters/{gpu-world,world-store}` and `tools/evidence`. Rust 1.99.0 is pinned in `rust-toolchain.toml`; `Cargo.lock` is tracked (wgpu 29.0.4, winit 0.30.13, egui, egui-wgpu and egui-winit 0.35.0, standalone `bevy_ecs` 0.19.0, redb 4.1.0, bitcode 0.6.9). The native client's resolved closure has no Bevy engine crate. | Builds with `--locked`; `check static` passes with an empty lint baseline. One advisory without a compatible fix (quick-xml, build-time only, through wayland-scanner) is accepted in `deny.toml` with its reason. |
| World model | `world-model` owns packed cells and their column rules, materials, the automaton's rules (`rules`), chunk keys, units, supported settings, body presets and tuning, commands (rays, targets, queries, configuration, totals), observations, the layout and physics fingerprints (`identity`), body resume records (`resume`) and generation. The runtime, GPU owner (through generated declarations), renderer, client and runner import it. | 41 tests, including hand-written layout goldens, the baseline hash written out by hand, independent ceiling fixtures, the substep and apportionment rules and the pinned fingerprints. |
| Runtime | `world-runtime`: identities, the `PhysicalWorld` port with per-tick accounting, bounded receipts, the outcome ledger, the body monitor, the fixed reconfigurable clock, tick-scheduled commands, the storage port and workers, the residency and checkpoint owners and `WorldSession` (open, save, autosave every 60 ticks by default, orderly close, loss report with the interval saves ran at), in a standalone `bevy_ecs` world. Its consumers are the native client, the GPU suites, `resident-world` and `cargo evidence session`. | 46 tests, including the residency, checkpoint and session owners against a model port and an in-memory store; exercised with the GPU owner in every GPU suite and the scenario. |
| GPU world | `gpu-world`: explicit adapter selection, requested limits, one submission timeline (≤ 3 world submissions in flight), checked resource plans up to radius 32, generated layouts, assembled WGSL validated offline, chunk slots, the material phases with sparse tile scheduling (03), bodies, rays and every body command (04), change words, chunk and world snapshots (the world image kept between saves since `21674ad`) and resize (05). Since 06 it opens a session presenting to a window (`open_presenting`, the adapter's backend only) and takes the renderer's frames on the same timeline with their own bound of two (`submit_presentation`); between frames it reports, without waiting, whether a frame or a tick would be admitted (`poll_admission`). | 34 tests. `transfers` (28 cases), `cells` (31), `bodies` (32) and `streaming` (23) pass on the RTX 5070 Ti on `baa068e`'s tree. Measured: an idle radius-6 window costs 27 µs of device time per tick, a fully active one 280 µs ([material phases](../reference/performance-budgets.md#material-phases-03)); the legacy qualification body workload's body pass takes 0.63 ms ([bodies](../reference/performance-budgets.md#bodies-04)). |
| Terrain | GPU only: the column rules as WGSL kernels over the device cells, every transfer proposed once and accepted against the receiver's capacity; idle tiles are neither visited nor copied, and immature foliage keeps its tile awake only while it can grow (after 07, physics revision 2); a window move keeps the kept chunks' activity and wakes only the rows it drops and loads (after 07). The CPU automaton was removed in 06; its ten simulation tests run as `cells` cases. | The CPU reference in `monarchy-evidence` matched the baseline engine cell for cell on its fixtures until the engine was removed, and the GPU equals the reference tick for tick in the `cells` suite. |
| Bodies | GPU only: fixed substeps of the tick, hashed bins, swept contacts, a speed cap of 600 units/s, shared deformation and exact material accounting. The CPU sphere integration was removed in 06; its twelve tests and the apportionment test run as `bodies` cases. | Measured on the frozen legacy workloads: the baseline's body time per tick was 5.32 ms (CPU, 64 spheres) and 16.99 ms (256); the GPU owner takes 0.63 ms and 0.98 ms of device time. |
| Material contracts | Column mixing, phase changes and capacity helpers in `world-model`; the per-operation conservation, sinks and capacity policy in the [GPU contract](../reference/gpu-contract.md#material-accounting-03), and the bodies' accounting ([bodies](../reference/gpu-contract.md#bodies-04)). | 5 baseline cell tests and 3 ceiling tests pass on the CPU; the WGSL rules equal them on 15,488 boundary columns on the device. Every GPU tick reports exact 64-bit accounting of cells and bodies that the ledger checks. |
| Rendering | `world-render` (06): the four landscape layers drawn per window chunk from the device cells (the baseline's landscape material, ported), spheres instanced from the device body state, GPU visibility (chunk bounds, frustum culling and ordered compaction into indirect draws), a camera that follows a body from its device pose, heightfield shadows and occlusion cached per chunk vertex and relit only for chunks whose cells or shadowing neighbours changed (since `63f8ce6`; 33.8 kB per window chunk, 142.8 MB at radius 32), a sphere shadow map, fog with sun glow, bloom and AgX tone mapping. It writes no physical state and keeps no copy of it. | 12 tests; the `render` suite's 15 cases pass offscreen on the RTX 5070 Ti on `67edbc2`'s tree, among them the GPU owner's `presentation-admission` and `lighting-cache-matches` (the cache within one 8-bit level of lighting every vertex, relighting nothing on an unchanged frame). Measured at 1920×1080 from the qualification camera: scene passes 0.46, 1.47 and 4.57 ms at radius 6, 16 and 32 (0.86, 4.30 and 14.84 ms lit per vertex, as before the cache; 0.66, 3.21 and 11.86 ms unculled), post 0.09–0.11 ms ([rendering](../reference/performance-budgets.md#capacity-after-07)). |
| Streaming/storage | `world-store` format version 1 (CRC-32C rows, immutable chunk and body revisions, atomic publication, stale-save refusal, reclaim after an unclean session), the legacy import, and the runtime's residency, checkpoint and session owners over the GPU owner; the client creates, opens, imports, saves, autosaves (every 60 ticks by default, 60–3,600, never off) and closes worlds through them; a crash or a lost device loses at most the interval and the save in progress while storage accepts writes; the `world-store` command and `cargo evidence session` remain for scripts. | The `streaming` suite (23 cases) with real stores, injected I/O errors and process interruption, among them `bounded-loss` (a device lost while a save copies lost 60 ticks, a process ended before a publication committed 63, failing storage reported and recovered) and `replay-reproduction`; `world-store`'s 37 tests of the format, the store, imports and the command against real files and the old writer's fixtures, among them damaged and foreign worlds refused unchanged. Measured: the ordinary resident simulation moves no world data, a save reads back only changed chunks ([streaming and checkpoints](../reference/performance-budgets.md#streaming-and-checkpoints-05)); saves every 60 ticks publish within 10 ticks, cost no frame time beyond the spread between runs and write 1.7–2.6 MB/s to storage under busy workloads ([checkpoint cadence](../reference/performance-budgets.md#checkpoint-cadence-after-07)). |
| Native client | `morbid-app` (06): a pumped winit loop that runs a frame only when one has work (a free presentation slot, a due tick the device can take) and otherwise waits in the event wait, an egui overlay (tool bar, simulation, lighting, statistics, world, outcome and pressure panels, compass), the world catalog and settings outside the working directory, explicit adapter choice, open/close/save feedback, an autosave interval of 60 to 3,600 ticks (default 60, never off) and the ticks since the latest checkpoint in the world panel, device-loss report (ticks lost and the autosave interval) and recovery under a later epoch, every inventory tool as runtime commands at GPU-resolved rays and column queries. | 9 tests; on `dfeb530`'s code the `loss` script passed its 6 checks and `worlds` its 5 in the real window (both save only when their script says, as on `2c63b64`), and the frozen `coupled` scenarios ran with `--autosave 60` at the frame times of autosave off ([checkpoint cadence](../reference/performance-budgets.md#checkpoint-cadence-after-07)); in its real window on `2c63b64` the `features` script passed 41 checks in each of three runs (every inventory group through the input path), `loss` 6 (save, loss, recovery from the latest checkpoint, the report on screen) and `worlds` 5 (create, import, refusal of a damaged save, close with save, reopen), as on `eb9d8fb` in 07. On `eb9d8fb` the copied executable started fresh outside the repository with an empty home directory, opened a world and closed it through the compositor with its save ([qualification](../development/qualification.md#packaged-fresh-start)). |
| Interaction | Every brush, sphere preset, drive, jump, attract and lift as commands at a column or a GPU-resolved ray, through the runtime's receipts; walking on column queries; statistics from GPU totals. | The `features` script checks each tool against its command outcome in the real window; GPU commands, rays and retries are also tested headlessly. |
| Event path | Command outcomes and residency results with reserved storage, a per-tick cosmetic event buffer whose drops are counted, body samples and per-body counters consumed by the runtime's body monitor, whose events are deferred, never dropped, under pressure; the client logs them. The baseline's unconsumed event queue went with it. | Delivery under a saturated ring, delayed and reordered samples and slot reuse is tested on the GPU (`observation-delivery`); `resident-world`'s consumer reacts to it, and its step 6 drains the events of 62 piled spheres after a long absence, every one adding up to the bodies' counters; not yet a gameplay notification system. |

The [qualification](../development/qualification.md#baseline-dispositions) records what the
baseline did and where each of its responsibilities went.

## Measured result

The Bevy baseline, measured at `6e108d6` on the nominated machine (Ryzen 9 5950X, RTX 5070 Ti,
Vulkan, KDE Wayland), five valid runs per case, missed the qualification budget in body-contact
(p95 18.7 ms) and coupled (p95 28.6 ms, p99 32.1 ms) and fell behind real time in every active
scale case (17–30 ticks/s). The native client, measured on 2026-10-22 with the same protocol at
`67edbc2`'s code (five valid runs per case of every profile, a clean tree, the person away from the
desktop; 07's qualification at `eb9d8fb` and the runs between are in the budgets):

| Profile | Frame median native / baseline | p95 native / baseline | Worst p99 | Ticks/s native / baseline | CPU per tick (simulation) | Grid transfer |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Qualification (radius 6, 64 bodies) | 0.64–0.80 / 3.57–17.12 ms | 0.9–10.6 / 6.1–28.6 ms | 10.72 ms | 59.4–60 / 60 | 0.07–0.28 / 2.1–13.1 ms | none / 0.60–0.85 GiB/s |
| Scale (radius 16, 256 bodies) | 1.64–1.89 / 27.0–223.8 ms | 1.7–3.3 / 36.7–252.6 ms | 3.55 ms | 59.8–60 / 17–60 | 0.30 with bodies / 7.6–51.6 ms | none / 0.76–3.15 GiB/s |
| Capacity (radius 32, up to 4,096 bodies) | 4.99–11.72 ms | 6.3–14.0 ms | 16.17 ms | 60 | – | none |

Every profile's p95s and p99s are inside the qualification budget (p95 ≤ 16.67 ms, p99 ≤ 25 ms),
which only the qualification profile promises; capacity admits its workloads and reports when it
falls behind (counted discarded ticks, pressure) instead of truncating physics, and since `10cf62f`,
`63f8ce6` and `67edbc2` it keeps 60 ticks/s with 4,096 dense bodies (medians before them 18.1–27.1
ms, worst p99 51.6 ms, 52 ticks/s). Seven frames of 53–478 ms in radius-6 and radius-16 coupled and
streaming runs waited for the device, a few ticks were discarded after them, and twelve alternating
runs with the screen lock off had none, with or without the window move; the person's screen may
have locked
([budgets](../reference/performance-budgets.md#the-three-profiles-after-the-window-move)). Without
vsync, as the frozen profiles present, some frames wait 10.09 ms for a swapchain image: 3–5% of the
frames at radius 6, so they set the worst p99 near 10.7 ms there and, in three body-contact runs
over 5%, the p95; they do not occur with vsync. The loop runs one frame per presented frame at
radius 6 (1.1 at radius 16, 1.3–1.5 at radius 32) and spends 0.5–0.6 ms of main-thread CPU per
presented frame at radius 6, 0.7–0.8 ms at radius 16 and 1.4–1.9 ms at radius 32. Simulation CPU per
tick is the encoding and submission of a tick and the session's owners, measured by the GPU suites.
The tables, conditions, transfers, memory, allocations and the capacity workload's limits are in the
[performance budgets](../reference/performance-budgets.md#integrated-qualification-07), the runs
since [after them](../reference/performance-budgets.md#the-three-profiles-after-the-window-move).

## Known gaps

- Without vsync, the KWin Wayland and Xwayland paths with the NVIDIA driver make some presented
  frames wait a constant 10.09 ms for a swapchain image: up to 22% of a run's frames in 07's
  measurements, at most 4.2% on `2c63b64` and 3–5.4% at radius 6 on `6e3f2d9` (none at radius
  16, at most 1.9% at radius 32), which set the worst p99 near 10.7 ms and, in three runs over 5%,
  the p95. The wait does not depend on the swapchain's length and does not occur with vsync;
  attributing it to the driver is an inference.
- The loop's 250 µs idle wait delays the fastest frames by about 0.1 ms at radius 6 (the frame
  median is unchanged).
- With 4,096 dense bodies at radius 32 the body pass takes about 5.1 ms of device time per tick; the
  window keeps 60 ticks/s at 10.6–11.7 ms frame medians. A window move was a full resize (a second
  window, every chunk copied, every cell woken) and made one 32–40 ms frame in each coupled run on
  `76376d8`; since `67edbc2` it happens in place and the longest capacity frame is 23.8 ms ([window
  moves](../reference/performance-budgets.md#window-moves)). Process GPU memory at radius 32 reaches
  1,236–1,296 MiB, the most at the closing save, whose world image is kept since `21674ad` until the
  world drops; the lighting cache reserves no memory beyond what the window already held. The
  capacity profile promises no rate.
- A crash or a lost device loses what came after the latest checkpoint: at most the autosave
  interval (60 ticks by default, up to 3,600 if a person sets it) and the save in progress, as long
  as storage accepts writes; while it fails, the exposure grows and the overlay says so. Nothing is
  replayed: a command journal is a parked
  [whiteboard topic](../development/virtual-office/whiteboard/discussions/command-journal.md). Saves
  write 0.3–2.6 MB/s to storage. A world whose format, layout, physics or generator identity
  differs is refused, not converted: before release saved worlds are test data, and no migration is
  written (the user's decision, 2026-10-21).
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
| Documentation | Passed (after 07) | `cargo evidence check docs`: links and anchors, owners, assignment structure, retired rules, package READMEs and their naming in the architecture; passed on `67edbc2`'s tree (in `check all`, 214 links). |
| Rust compilation, linting and dependency policy | Passed (after 07) | `cargo evidence check static` on `67edbc2`'s tree (in `check all`): rustfmt, two feature combinations, Clippy with no exceptions, rustdoc with warnings denied, naga validation of the assembled `gpu-world` and `world-render` shaders, `cargo deny`, `cargo machete`, dependency-direction rules per package (only the standalone ECS crates in any closure), pins and CI. |
| CPU tests | Passed (after 07) | `cargo evidence test cpu` on `67edbc2`'s tree (in `check all`): 241 tests, checked against the registry's 182 CPU behaviors and 61 runner tests; the 28 baseline behaviors stay registered, 23 of them as GPU cases. |
| GPU correctness and buffer layout | Passed (after 07, every suite) | `cargo evidence check all --adapter vulkan-10de-2c05` on `67edbc2`'s tree (162 of 162 items, 2026-10-22 08:27–08:33 UTC, `target/evidence/move-check-all/`): `transfers` (28 cases), `cells` (33), `bodies` (33), `streaming` (23), `render` (15) on the RTX 5070 Ti, NVIDIA 615.78.08, Linux 7.2.11. |
| Connected scenario | Passed (headless after 07; native in 07) | `cargo evidence run --scenario resident-world --adapter vulkan-10de-2c05`: all six steps, step 6 the adversarial repeat, headless on `67edbc2`'s tree (in `check all`) and with `--consumer native` on `eb9d8fb`'s (07; its viewer has its own loop, which `2c63b64` did not change). |
| Native interactive client | Passed (after 07, scripted in the real window) | `native-client script features` (41 checks), `loss` (6) and `worlds` (5) on `6e3f2d9`'s tree, `67edbc2`'s code (`target/evidence/move-scripts/`); `features` three times on `2c63b64`; the packaged fresh start outside the repository on `eb9d8fb` (07). |
| Primary hardware performance | Qualified (07), measured again after it | `cargo evidence bench --profile qualification`, `scale` and `capacity` with `--runs 5` at `eb9d8fb` (07), compared with the baseline's summaries in `tools/evidence/baselines/legacy-6e108d6/` ([budgets](../reference/performance-budgets.md#integrated-qualification-07)), and at `2c63b64` against 07 ([budgets](../reference/performance-budgets.md#client-loop-without-vsync-after-07)); radius 32's tick phases and scene passes headlessly on `d4d2d6c` and `capacity` with `--runs 5` on `76376d8`'s tree after them ([budgets](../reference/performance-budgets.md#capacity-after-07)); all three profiles on `b766ae9`, 119 of 124 attempts rejected for the window's size (KWin's placement), and again on `6e3f2d9`, every attempt valid ([budgets](../reference/performance-budgets.md#the-three-profiles-after-the-window-move)). |
| Fault recovery and legacy save import | Passed (07; the loss bound after it) | `streaming` cases, since `2932764` `bounded-loss` at the client's autosave cadence (a lost device, an interrupted process and failing storage, each within or reporting the bound); `resident-world` step 6 (failed writes, a failed and retried save, device loss, process interruption before and after publication, orderly close, fresh reopen); `world-store` tests, since `b7d86bb` damaged and foreign worlds refused before the file is opened for writing; the `loss` and `worlds` scripts (on `2c63b64`); a damaged legacy save refused by the start screen and by `world-store import-legacy`. |
| Secondary GPU / Windows / other platforms | Unqualified | No compatibility claim. Hosted CI runs the CPU and static lanes on Linux only. |

Replace affected rows with the tested commit, command, environment and result when later work
produces evidence. Keep detailed logs in artifacts, not this page.

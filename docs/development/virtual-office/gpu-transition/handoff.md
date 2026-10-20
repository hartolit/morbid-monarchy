# GPU transition — current handoff

Maintain this one current record; replace stale working notes rather than accumulating a session
transcript. Canonical design/status belongs in maintained docs, and raw evidence belongs in retained
artifacts. Update after each coherent checkpoint and before stopping or changing context.

## 07 (in progress)

**State:** 07 assigned on 2026-10-20 and in progress. Checkpoint A's gate ran on `93774e0`
(every lane passed except step 6, as designed). Checkpoint B's `resident-world` step 6 is
complete and committed in `d9bffb6` (both consumers). Visible runs (scripted windows, the
packaged fresh start, the benchmarks) are on hold until the user says go: a game was using the
GPU on 2026-10-20 from about 20:05 local time.

| Field | Record |
| --- | --- |
| Source state | `experimental` at `d9bffb6`, plus uncommitted work in the tree: the session's frame times by owner (`world_runtime::session::FrameTimes`), thread-local allocation counts and their use in `streaming-measure`, the surface's acquisition wait, the benchmark's whole-process account of the measured window (`native::bench`), `native-client` counting its allocations and a diagnostic `--autosave`, the `worlds` script refusing a damaged legacy save (optional default directories), and the client loop no longer laying out the overlay in iterations that cannot present. Drafts of the maintained documents, the permanent summary and the analysis scripts are under `target/drafts/07/`. |
| Checks so far | `93774e0`: `check all` 151 of 152 (step 6 only). `d9bffb6`'s tree: `check docs`, `check static`, `test cpu` (232 tests); the tree before its last three edits: `check all` 150 of 152 (rustdoc link and runner test, fixed) and `run --scenario resident-world --consumer native` passed. Uncommitted tree: `world-runtime` tests (44), Clippy, one headless `streaming-measure`. |
| Findings | Measured on a contended machine (indicative only): the client loop ran about 10 iterations per presented frame without vsync, each laying out the overlay (about 2 ms of main-thread CPU and 4,400 allocations per presented frame); fixed in the uncommitted tree, to be measured. Saving's extra frame cost is in the checkpoint owner and the store's worker threads, walking's in the residency owner and the workers. |
| Next action | When the user says go: scripted window runs (`features`, `loss`, `worlds`), the packaged run (`target/drafts/07/packaged.sh`), acquisition-wait experiments; commit the instrumentation; the five-run benchmarks of qualification, scale and capacity on that commit; documents; the final gate; closure. |

## 06 boundary (closed)

**State:** 06 complete (2026-10-20), assigned on 2026-10-19. Checkpoints A–E are committed on
`experimental`; every lane passed on the nominated hardware except `resident-world`'s step 6,
which 07 completes. The checks row names the commits.

## 06 record

| Field | Record |
| --- | --- |
| Assigned stage / checkpoint | 06 — Native client and Bevy removal. A–E complete. |
| Source state | `experimental`: `1afad00` (A–C first pass), `199f25c`, `f57d42a` (B: culling, the `render` suite), `22be809` (C, E: start screen and device loss scripted), `6f3b1a4` (E: `resident-world` in the window), `9a3230c` (E: the native benchmark), `a70ce1c` (D: removal), `5bf7b44` (fixes the lanes found), `da1c3ba` (the script cursor), then this record. Work began at `d174e0f` (05's record); this record's tree has no other uncommitted changes. |
| Actual adoption | **A:** `apps/morbid` is `morbid-app` (library `morbid_app`, executable `morbid`): a winit 0.30 window driven by `pump_app_events`, an egui 0.35 overlay, the world catalog (create, import, list), settings and options outside the working directory, explicit adapter choice among adapters that can present, `WorldSession` over `world-store` with open, save, autosave interval, orderly close, loss report and recovery under a later epoch, every inventory tool as runtime commands at GPU-resolved rays and column queries. `gpu-world`: `open_presenting`, presentation submissions on the world's timeline with their own bound of two, presentable adapters, a session instance of the adapter's backend only. `world-runtime`: `PhysicalWorld: Any` and `Runtime::port_as`, `ticks_due`, clock settings, a settable autosave interval. **B:** `world-render`: the baseline's landscape layers ported and drawn from the device cells, instanced spheres from device body state, the follow camera computed on the device, heightfield shadows and occlusion, a sphere shadow map, fog, bloom, AgX; GPU visibility (bounds, frustum culling, ordered compaction, indirect draws), optional pass timestamps. **C:** the start screen's create, import, open, close and reopen; device loss and recovery in the window; one surface per window. **D:** deleted the Bevy baseline application (with its persistence, plugins, materials and shaders), `monarch-engine`, `spatial-lib`, the `legacy-baseline` driver and feature and the CPU reference's test against the engine; dropped bevy, bevy_egui, lru, rayon, flume and rustc-hash; re-pointed the 23 engine-backed baseline ids to their GPU cases (and the clock's to the runtime's clock test); emptied the lint baseline; retired the lru, ruzstd and ttf-parser advisory exceptions (winit's no-title decorations drop ttf-parser); CI installs no Bevy headers. **E:** the `render` suite (12 cases); `native-client` scripts (`features`, `loss`, `worlds`, `capture`); `resident-world --consumer native` through `morbid_app::viewer`; `cargo evidence bench` measures the native client (`native-client bench`, one sample per presented frame). |
| Decisions / deviations | The client is one window with one surface at a time: a session's instance holds only its adapter's backend, opening a world is refused while another closes, and a closing world's frames finish before its surface goes (KWin's Wayland refuses a second surface's tearing control and drops the connection). Presentation never waits for the device: with both presentation slots busy the client skips drawing that iteration, and the timeline keeps running. Without vsync the client prefers mailbox to immediate (measured: immediate waited 10.09 ms in image acquisition on every presented frame, mailbox on about 15%). Tone mapping is AgX (TonyMcMapface needs a lookup texture); culling was added after measuring the unculled renderer (1.81 ms at radius 6, 11.75 ms at radius 16) and is pixel-identical to it. The benchmark samples presented frames; loop iterations that present nothing belong to the next sample. A drive action possesses the body whose spawn outcome is nearest the camera at its first tick (deterministic), where the baseline driver used the bodies' current positions. The CPU reference's agreement with the deleted engine can no longer run; the reference stays as the `cells` suite's oracle. Found and fixed: a refused adapter id listed only its own backend's adapters; a session test (05) raced with autosaves during settling (10 failures in 300 runs before, none after); scripted cursors could be moved by the person's real pointer over the window. |
| Checks | `a70ce1c`'s tree (2026-10-20 12:11–12:15 UTC): `check static` failed on one rustdoc link, `test cpu` on the registry and the racing session test, `transfers` on `adapter-selection`; `cells`, `bodies`, `streaming` and `render` passed; fixed in `5bf7b44`. `5bf7b44`'s tree (12:17–12:36 UTC): `check docs`, `check static`, `test cpu` (232 tests; 177 CPU behaviors, 57 runner tests), `test gpu --suite transfers`, `cells`, `bodies`, `streaming`, `render` passed; `run --scenario resident-world` headless and `--consumer native` passed steps 1–5 and failed step 6 only on its "07 completes this" check; `native-client script features` (41) and `worlds` (4) passed, `loss` failed 3 of 6 (fixed in `da1c3ba`); the benchmark below. `da1c3ba`'s tree (12:37–12:45 UTC): the same lanes with the same results, all three scripts passed (51 checks), and the benchmark's three remaining cases. Reports in `target/evidence/06-*`, `06f-*` and `06g-*`; script reports in `target/drafts/06/final*/`. Each tree was tested as committed, with `CARGO_BUILD_JOBS=4`. This record changes documentation only; `check docs` on it is in its commit message. |
| Hardware / measurements | RTX 5070 Ti (`vulkan-10de-2c05`, NVIDIA 615.78.08), Ryzen 9 5950X, Linux 7.2.11, KDE Wayland. Renderer offscreen at 1920×1080 from the qualification camera: scene passes 0.84 ms at radius 6 (66 of 169 chunks a layer), 4.20 ms at radius 16 (355 of 1,089), 1.81 ms and 11.75 ms unculled; post 0.09–0.10 ms; encoding 0.18 ms and 0.26 ms of CPU; 400 B of uniforms a frame, one world bind group. The native client in its window, one valid run per qualification case (idle, streaming, coupled at `5bf7b44`; flow, body-contact, mixed at `da1c3ba` after KWin shrank all three attempts of each to 1920×1052 in the first run): frame medians 1.06–1.23 ms, p95 10.63–10.74 ms, p99 10.85–10.99 ms, no frame over 16.67 ms, 60 ticks/s, CPU about 0.46 ms a frame, tick device time 0.19–0.88 ms (body pass 0.64–0.66 ms with 64 bodies), RSS 291–365 MiB, GPU memory 241 MiB. Every case meets the qualification budget in that run; p95 is set by 10.09 ms waits in swapchain image acquisition on about 15% of frames. Details: [performance budgets](../../../reference/performance-budgets.md#native-client-06). |
| Acceptance | **A10 satisfied:** the renderer draws the GPU owner's buffers on its device and timeline (`render` suite: formats, zero size, terrain, missing chunks, edits next frame without new resources, spheres and the follow camera at their device pose, bind groups only on buffer replacement, device loss, culling against the unculled path, frame cost), the real surface runs (`features`: resize, zero size and restore; `loss`), `resident-world` in the window, startup with no assets on disk. **A11 satisfied:** every inventory group through the real input path with its command outcome (`features`), save, recovery and pressure feedback (`loss`, `worlds`, the world and outcome panels), no CPU edit path. **A12 satisfied:** the native closure has no Bevy engine crate (`check static`'s dependency rules, `the_native_client_closure_has_no_bevy_engine_crate`), the baseline application, CPU engine and chunk manager are deleted with no fallback, and the CPU reference is development-only. A13–A15 are 07's. |
| Data safety | Every case, script, scenario and benchmark used temporary stores under `target/`; the legacy fixture was copied before the `worlds` script imported it. No 06 command opened `world_data/save.redb` (the user said on 2026-10-19 that it is a generated save with nothing important). The client keeps its worlds in the XDG data directory. |
| Blockers | None for 07. |
| Next action / stop | Stopped at the end of 06. Assign 07. |

### Carried into later stages (06)

1. **Acquisition waits** (07): on KWin Wayland with the NVIDIA driver, about 15% of presented
   frames wait a constant 10.09 ms in `get_current_texture` (mailbox), which sets the native
   frame's p95 near 10.7 ms in every case; the display intervals here are 4.2 and 6.9 ms, so it is
   not refresh pacing. Find out whether the compositor or the driver holds the images, and
   measure on X11 or another compositor.
2. **The full native qualification** (07): five or more runs per case and profile, the
   comparison with the baseline's measurements (the baseline sampled its main schedule, the
   client samples presented frames), and the scale and capacity profiles in the window.
3. **`resident-world` step 6** (07), unchanged from 05: delayed consumers, dense body work at
   chunk seams with phase changes, a negative-coordinate rebase, in both consumers.
4. **The client's loop** iterates without presenting while both presentation slots are busy
   (about four iterations per presented frame in the idle benchmark); it does no blocking wait,
   but each iteration lays out the overlay. Judge its CPU cost in 07.
5. **Frame cost of saving and streaming** (from 05): still not broken down by source.

## 05 boundary (closed)

**State:** 05 complete (2026-10-19), assigned by the user on 2026-10-18. Checkpoints A–D are
committed on `experimental`; every lane passed on the nominated hardware except
`resident-world`'s step 6, which 07 completes. The checks row names the commits.

## 05 record

| Field | Record |
| --- | --- |
| Assigned stage / checkpoint | 05 — Streaming, checkpoints and recovery. A–D complete. |
| Source state | `experimental`: `f01c71a` (A–B), `9c104e4` (C), `6f6297c` (D's code and the measurement-independent documents), then this record with the measurements, status and acceptance. Work began at `a6c74d7` with a clean tree; no unrelated work was present. |
| Actual adoption | **A:** `gpu-world` change words (every cell writer marks its slot; activation and snapshots clear it; resize carries it), the fixed chunk snapshot image and the on-demand world image within the plan, flag sheets so unchanged chunks are never read back, transfer-only submissions, body export and restore, resize preflight. `world-runtime`: the storage port (`store`), load and write workers, the residency owner (per-key phases, request generations, newest retained and durable revisions, byte budgets, pressure, stale and out-of-order loads discarded, absent / corrupt / unsupported / I/O failures kept apart). `world-model`: `identity` (layout and physics fingerprints), `resume` (body slots and resume records), `SimulationRules::from_bits`. **B:** `world-store` (format version 1, the redb store with atomic publication, stale-save refusal, idempotent retries, reopening after I/O errors, session markers and reclaim; fault injection under redb and at barriers), the checkpoint owner (one world snapshot at a completed tick, durable revisions, bodies, publication, retries, receipts) and `WorldSession`. **C:** the legacy reader and non-destructive import (`world_store::legacy`, moved from `spatial-lib` and `morbid-app`, with the old writer's fixtures), the `world-store` command, orderly close, loss reports and recovery under a new epoch, the headless `cargo evidence session`. **D:** the `streaming` suite's 19 cases with real temporary stores, `resident-world` steps 4–5 and 05's part of step 6 through `WorldSession`. Superseded: `spatial-lib`'s storage module; the baseline load task's generation over unreadable rows. |
| Decisions / deviations | Change detection is on the device (one word per slot), not a CPU dirty flag, so a snapshot can tell an unchanged chunk without reading it. A checkpoint is one world snapshot at a completed tick plus the immutable revisions of chunks that were not resident; inactive chunks stay frozen. One redb file per world, `Durability::Immediate`; chunk rows and body sets are written ahead and one transaction publishes. A failed sync is treated as ambiguous: retries of the same operation are idempotent. A world whose layout, physics or generator identity differs is refused, not converted. Import requires the seed and elevation scale, refuses `z ≠ 0` rows unless told to skip them, and repairs a copy of an unclean original. Close publishes a checkpoint holding the last completed tick or reports that it did not; a session that did not close leaves the store unclean and the next one reclaims what it left. No command journal: changes after the last checkpoint are lost on a crash or device loss, and the loss report says how many ticks. The headless session command is 05's consumer of store selection, saving and reopening; the application's options are 06's. Behavior change in the baseline: an unreadable row is left unloaded instead of generated over. Found and fixed in C: a store opened for writing changed the bytes of a file it then refused as not a world (the old writer's save among them); a close judged its save by the request's reason, reporting a committed close unsaved during an autosave and taking no save of its own after an earlier one. `resident-world` step 6 runs 05's part and stays failed; the six steps of the acceptance trace are unchanged. A reboot (kernel 7.2.10 to 7.2.11) interrupted C's lanes, which were rerun from the start. |
| Checks | `f01c71a`'s tree (2026-10-18 22:06–22:18 UTC, Linux 7.2.10): `check docs`, `check static`, `test cpu` (231 tests; 180 behaviors, 56 runner tests), `test gpu` `streaming` (13 cases), `transfers`, `cells`, `bodies`, all passed. `9c104e4`'s tree (2026-10-19 12:34–12:39 UTC, Linux 7.2.11): `check docs`, `check static`, `test cpu` (235 tests; 183 behaviors, 57 runner tests), `streaming` (17 cases), all passed. `6f6297c`'s tree (12:42–12:49 UTC): `check docs`, `check static` (Clippy 7 diagnostics, all covered by the 3 recorded exceptions), `test cpu` (235 tests; 183 behaviors, 57 runner tests), `test gpu --suite streaming --adapter vulkan-10de-2c05` (19 cases), `transfers` (29 items), `cells` (32), `bodies` (33) passed; `run --scenario resident-world` failed as intended: steps 1–5 passed, step 6 failed only on its "07 completes this" check. Reports in `target/evidence/05ab-*`, `05c-*` and `05d-*`. Each tree was tested as committed, with `CARGO_BUILD_JOBS=4`. This record changes documentation only; `check docs` on it is in its commit message. |
| Hardware / measurements | RTX 5070 Ti (`vulkan-10de-2c05`, NVIDIA 615.78.08), Ryzen 9 5950X, Linux 7.2.11, on `6f6297c`'s tree: the ordinary resident simulation at radius 6 with every rule on uploads and reads back no world data (1,312 B read back a tick, its own records); a save of the resident window copies 169 chunks on the device and reads back the 112 that changed, committing 26 frames after the request; walking, each entered chunk uploads 32,768 B, moves settle in 5 frames, loads wait at most 0.88 ms and writes 9.1 ms, retained payloads peak at 3 MiB; saving and walking add about 0.4 ms at p95 and 190–350 allocations to a frame's CPU time against 0.11 ms and 119 resident; no frame or maintenance wait. A world snapshot draining through a resize peaks exactly at the plan's overlap bound. Details: [performance budgets](../../../reference/performance-budgets.md#streaming-and-checkpoints-05). Headless only: no native frame time. |
| Acceptance | A08 satisfied: metadata-only residency (`world_runtime::residency` holds no cells; `resident-no-readback`), capture before reuse (`delayed-mapping`, `newest-retained-wins`, step 4), reordered and stale loads (`reordered-and-stale-loads`), a dirty chunk returning before its save (`newest-retained-wins`, step 4), resize overlap (`snapshot-resize-overlap`, step 6), bounded retries and retention (`failed-commits-retained`, `retention-budget`), body suspension and resume across saves (`body-suspension-resume`, `snapshot-restore-bodies`, step 4), closed missing neighbours at negative-key corners (`missing-neighbours-closed`). A09 satisfied: resident-only autosave and periodic saves, chunk, body and configuration capture at one tick (step 5's byte-for-byte re-save), atomic publication under interruption (`store_lifecycle`, `process-interruption`), fresh reopen by new store objects, refusal of corrupt, truncated and future rows (`refused-rows-stay-out`), import of the old writer's fixtures (`import`, `cli`), orderly close with work in flight and its timeout (`orderly-shutdown`), and the loss boundary before and after capture (`loss-and-recovery`). A10–A15 not run; A14's complete trace is 07's. |
| Data safety | Every case, the scenario and the session runs used temporary stores or scratch copies; the legacy fixtures are only read, or copied before an import or a session opens them. `world_data/save.redb` was not opened by any 05 command: it is 8,266,977,280 bytes, modified 2026-10-17 18:39:53 local time, as 04 left it, checked at each checkpoint. Since C a store opened for writing refuses a file that is not a version-1 world before opening it for writing, so pointing a store option at the legacy save leaves it unchanged. |
| Blockers | None for 06. |
| Next action / stop | Stopped at the end of 05. Assign 06. |

### Carried into later stages (05)

1. **The native client** (06) composes `WorldSession` over `world-store`: world selection,
   creating and importing worlds (the user decides whether and when to import
   `world_data/save.redb`, from a copy), save and close feedback, and loss and recovery
   feedback. It retires the baseline's legacy persistence, `spatial-lib`'s grid and chunk
   manager, and the headless session command's role as the only consumer.
2. **`resident-world` step 6** (07): delayed consumers, dense body work at chunk seams with phase
   changes, a negative-coordinate rebase and the native consumer, on top of 05's faults.
3. **Frame cost of saving and streaming** (06–07): about 0.4 ms at p95 and 190–350 allocations a
   frame, not yet broken down by source; judge against the frame budget.
4. **Format evolution** (whenever a layout, rule or body constant changes): there is no
   conversion; a changed fingerprint refuses older worlds. A migration must exist before such a
   change ships to a user with saved worlds.
5. **Autosave interval** (06): the session's default is 3,600 ticks (one minute at 60 Hz); the
   client chooses and exposes it.

## 04 boundary (closed)

**State:** 04 complete (2026-10-18), assigned by the user on 2026-10-17. Checkpoints A–D are
committed on `experimental` and every lane passed on the nominated hardware; the checks row
names the commit.

## 04 record

| Field | Record |
| --- | --- |
| Assigned stage / checkpoint | 04 — GPU bodies and useful interaction. A–D complete. |
| Source state | `experimental`: `49c3f6b` (A–B: identity, clock, bins, solver, contact, deformation, the first 24 `bodies` cases), `2975112` (C: rays, observation delivery, four cases), `e8ee688` (D: measurement cases, per-body workgroup gather, `resident-world` steps 1–3), the documentation commit (the scenario aligned with the acceptance trace, these docs), then this record. Work began on a clean tree at `9506de9`; no unrelated work was present. |
| Actual adoption | **A:** `world-model` `body` (handles, descriptions and the drive envelope, the sphere tuning `monarch-engine` now reads, `substeps_per_tick`, the reach, `apportion` and its exact multiply-divide), `command` (rays, targets, every body command, `SetElevationScale`, `QueryRay`) and `observation` (`BodySample`, counters, ray hits). `world-runtime`: the tick rate and the bodies to sample in `TickBatch`, `BodyReport` and its outcome, the body monitor, ledger routing. `gpu-world`: the slot table, body records and plan (`4·(92 B + 26 S)` bytes, bins, claims `4 N`), substeps after the material commit, complete hashed bins with an all-pairs comparison mode, suspension at the residency edge, the speed cap. **B:** terrain contact, the bounded Jacobi solver, deformation under per-cell claims with exact apportionment, buoyancy and displacement, accounting with the compaction sink, preflight before mutation. **C:** GPU-resolved rays and ray-targeted commands, every body command, readback validation of ray answers, the monitor's lossless delivery. **D:** the four measurement cases, the per-body workgroup pair passes, `resident-world` through `gpu-suite --scenario`. Superseded: `BodyObservation` and `BodyRecord` (now `BodySample` and `BodyState`), `NotSupported` refusals of body commands, the overwriting event ring. |
| Decisions / deviations | Solver: per-body Jacobi with fixed-point sums so bins and all pairs agree bit for bit; position passes split overlaps equally and move a body at most 0.5 units a substep from where it was binned, which also bounds the candidate margin; velocity passes target a parting speed with the ground's settle threshold (resting pairs do not bounce); a body touching terrain slides along it. The default passes rose to 16 + 16 from the 8 + 4 first tried, on `dense-cluster` evidence. Convergence limit, declared from measurement: settled, no pair overlaps by more than 0.4 of its smaller radius (0.31 measured); a first draft bound of 0.4 units at the final tick, never committed, was not met and was replaced with this one and the reasons in the [discussion](../whiteboard/discussions/dense-pile-convergence.md). Residency edge: suspension (frozen, an obstacle, controls ignored); 05 persists it. The body report travels as its own outcome (`BodiesCompleted`): with it inside `TickReport`, `Outcome` grew to 376 bytes, and boxing would allocate every tick. Event ring: deferral instead of overwriting, so important body events are not lost under pressure. Found and fixed: the ray kernel's "no body" sentinel went through `f32` and reported a phantom body on every ray; the dense pile spent over 90% of its body pass in pair passes run one thread per body (7.6 ms at the qualification workload, now 0.64 ms). Upstream: 03's `cells` `presentation-rates` harness started its script at a timing-dependent tick and failed intermittently (4 of 4 runs at `9506de9` in a scratch worktree); it now starts at an aligned tick (`49c3f6b`). Rule changes from the baseline are listed in the [GPU contract](../../../reference/gpu-contract.md#bodies-04). |
| Checks | Final, at `71fb47f` with a clean tree (this record commit adds only this row and a corrected traffic sentence in the budgets), 2026-10-18 10:56–11:01 UTC, `CARGO_BUILD_JOBS=4`: `cargo evidence check docs` passed (5 items, 172 local links in 51 files); `check static` passed (12 items, 5 assemblies validated, Clippy 7 diagnostics all covered by the 3 recorded exceptions); `test cpu` passed (184 tests: 133 registered behaviors, 28 of them baseline, and 56 runner tests); `test gpu --suite transfers --adapter vulkan-10de-2c05` passed (29 items: the registry and 28 cases); `--suite cells` passed (32 items); `--suite bodies` passed (33 items: the registry and 32 cases); `run --scenario resident-world` failed as intended: steps 1–3 passed, steps 4–6 failed as not implemented (05, 07). Reports in `target/evidence/final-04-{docs,static,cpu,transfers,cells,bodies,resident-world}/`. Earlier: every lane except the scenario on `e8ee688`, clean (the measurements of record). `49c3f6b`'s tree before the commit: `check static`, `test cpu`, the `transfers` and `cells` lanes (the `cells` run reported only `identity.stable` failed, because the tree was staged during it; all 31 cases passed) and its 24 `bodies` cases run one by one through `gpu-suite`. `2975112`'s tree: `test cpu`, rustfmt and Clippy, and its 28 `bodies` cases through `gpu-suite`. |
| Hardware / measurements | RTX 5070 Ti (`vulkan-10de-2c05`, NVIDIA 615.78.08), Ryzen 9 5950X, Linux 7.2.10, at `e8ee688`, clean: the legacy body workloads take 0.64 ms (64 spheres, qualification) and 1.00 ms (256, scale) of device time per tick against the baseline's 5.32 and 16.99 ms of CPU time; 4,096 resting bodies 0.95 ms; a dense attracted pile of 4,096 bodies 8.2 ms with the bins and 97 ms with all pairs; paced at 60 Hz, 4,096 dense bodies keep 60 ticks/s with no frame wait, at most three in flight and body samples at most two ticks old; encode 0.19–0.36 ms; 336–347 process allocations per tick on the submit path in wgpu's command recording, 3 per poll. Details and resources: [performance budgets](../../../reference/performance-budgets.md#bodies-04). Per-case JSON in `target/evidence/<run>/gpu-bodies/` on that machine only. Headless physics only: no native frame time. |
| Acceptance | A06 satisfied: the twelve baseline sphere intentions and apportionment as GPU cases, the broadphase against an exhaustive oracle (dense, mixed-radius, coincident, bin-edge, fast), clusters and fast bodies, simultaneous deformation with brushes on the same cells, edits under a resting body, a bounded solve with declared limits and the suspension policy. A07 satisfied: spawn, removal, drive, jump, attract, lift, edits and queries at columns and GPU-resolved rays through runtime receipts (retry, conflict, expiry, capacity and stale handles), the body monitor and `resident-world`'s consumer reacting to observed immersion, deferral under a saturated ring with hostile replays, counted cosmetic overflow, and observation age at most two ticks. A08–A15 not run. |
| Data safety | No case or scenario opens a store; every world is synthetic or generated in memory. `world_data/save.redb` was written between the end of 03 (2026-10-17 17:59 local time) and the start of 04 (18:41): 04 found it at 8,266,977,280 bytes, modified 2026-10-17 18:39:53 local time, 72 seconds before this stage's first command. The session's commands were searched: none opened `world_data` or ran the application, and the file was unchanged at the end of 04. The earlier record (8,237,617,152 bytes, 2026-10-11 14:24) describes the file before that write. |
| Blockers | None for 05. |
| Next action / stop | Stopped at the end of 04. Assign 05. |

### Carried into later stages (04)

1. **Suspended bodies** (05): a body at missing terrain is suspended on the device only; 05
   persists it with its chunks, reloads it and resumes it, keeping its handle or replacing it
   explicitly.
2. **Checkpointed body state** (05): bodies (state, configuration, counters, the slot table's
   generations), the elevation scale, the broadphase and solver passes belong in the checkpoint;
   the counters are 32-bit and wrap, and the monitor's watermarks rely on that.
3. **`resident-world` steps 4–6** (05, 07): window movement and return, checkpoint and reopen,
   the adversarial repeat. The scenario reports them failed until then.
4. **Presentation from device state** (06): poses come from `BodyState` on the device, not from
   samples; possession maps to `Runtime::set_controlled`, the camera follows without a blocking
   readback.
5. **Submit-path allocations** (06–07): about two per dispatch inside wgpu (the body pass issues
   about 70 dispatches per substep); fewer, larger dispatches would reduce them if the frame
   budget needs it.
6. **Dense piles** (whiteboard, parked): [convergence of the body solver](../whiteboard/discussions/dense-pile-convergence.md).
7. **The baseline engine** (06) keeps its CPU bodies until the application moves to the GPU
   owner; the shared tuning keeps them in step.

## 03 boundary (closed)

03 completed on 2026-10-17; its record follows.

## 03 record

| Field | Record |
| --- | --- |
| Assigned stage / checkpoint | 03 — GPU-resident cell simulation and sparse work. A–D complete. |
| Source state | `experimental`: `c14fbea` (A: specification, `world_model::rules`, ceiling fix, CPU reference), `113b184` (B–C: GPU material phases, sparse scheduling, `cells` suite), `66ce775` (D: measurement case, keep-alive correction, documentation), then the hardware commit: the `presentation-rates` harness waits for the device between simulated frames, `steady-state-reuse` reports the command pass and the whole tick separately, and these records. The tree was clean before 03; no unrelated work was present. Another user process (`cargo-report`/`milkdrift`, from another project) ran during the first session and was not touched. |
| Actual adoption | **A:** the pass/resource table, activity scheduling, clock, keyed randomness, accounting and rule changes in the [GPU contract](../../../reference/gpu-contract.md#cell-phases-03), written before any kernel. `world_model::rules` owns rule switches, cadences, tuning constants, wake levels, the keyed hash and shuffles; the baseline engine reads them instead of its copies (its 23 tests unchanged). `WorldCell::quench` converts only what fits and `receive_fluid` refuses coolant at the ceiling without recursion (`receivable` answers without changing the column). New commands `SetSimulationRules` and `MeasureTotals`, observation `WorldTotals`. The CPU reference (`tools/evidence/src/suites/cells/reference.rs`, baseline gather form) equals `monarch-engine` cell for cell and wake level for wake level on its fixtures. **B–C:** `gpu-world` runs a tick as three compute passes (commands; scan, wake and two argument kernels; propose, resolve, react, commit) over 8 × 8 tiles with GPU-side lists and indirect arguments split at 65,535 workgroups; WGSL rule modules mirror world-model and material tables are generated from it; per-tick 64-bit accounting, totals and per-pass timestamps come back in the tick readback; dense execution is a configuration (`Execution::Dense`). `world-runtime` gained `CellReport`, the inconsistent-tick count in the ledger, `submit_at` and `set_clock`. **D:** the `cells` suite (31 cases) and the activity measurement. Superseded: the edit path that woke nothing (02), the full-grid copy as the only way to keep sleeping cells valid (the GPU path commits awake cells only). |
| Decisions / deviations | Activity keeps the baseline's per-cell wake levels and radii exactly (scheduled per tile, evaluated as a gather), so the deterministic rules reproduce the baseline with seed 0; tile-granular activity was rejected because it would have changed which cells run their rules. Transfers are proposed once and accepted against the receiver's worst-case column, so donors drain exactly what was accepted. Biology and fire use keyed draws (a semantic change). A rule change wakes every cell (the baseline did not). Immature foliage and fire keep their cell awake only while their rule runs: found when generated terrain never slept (64% of tiles at radius 6) under the deterministic rules. Captures now hold the end of the tick. The injected-magma behavior of the mixed scenario is kept; its ceiling loss is gone. The transfers suite opens worlds with every rule off, so its arbitrary-bit fixtures stay intact. RUSTSEC-2026-0344 (`ruzstd` under Bevy, published after 02, no compatible fix, no zstd texture loaded) is accepted in `deny.toml` until 06. |
| Checks | Final, on the hardware commit's tree before this record was written (one dirty-diff digest, `e8df6ae7…`, over `66ce775` for all four runs; the commit adds only this record), 2026-10-17 15:52–15:58 UTC, `CARGO_BUILD_JOBS=4`, host load average about 3 on 32 threads: `cargo evidence test gpu --suite cells --adapter vulkan-10de-2c05` passed (32 items: the registry and 31 cases); `--suite transfers` passed (29 items: the registry and 28 cases); `cargo evidence check static` passed (12 items, 3 assemblies validated, Clippy 7 diagnostics all covered by the 3 recorded exceptions); `cargo evidence test cpu` passed (166 tests: 115 registered behaviors, 28 of them baseline, and 56 runner tests); `cargo evidence check docs` passed at the commit. Reports in `target/evidence/final-03-{cells,transfers,static,cpu,docs}/` (copied from the session scratchpad). Earlier: `c14fbea` with docs, static and `test cpu` (161 tests) on a clean tree; `113b184`'s tree with static, `test cpu` (166 tests) and both suites on llvmpipe (`transfers` failing only its two hardware-only cases there); `66ce775` with static and docs. The first hardware run of `cells` failed `presentation-rates` alone: its simulated frames came faster than the device finished ticks, so backpressure left ticks owed and the 30 Hz run hit the cap; the harness now lets each frame's ticks complete, and the case also checks 20 discarded ticks per stall. No rule or kernel changed after `66ce775`. |
| Hardware / measurements | RTX 5070 Ti (`vulkan-10de-2c05`, NVIDIA 615.78.08), Ryzen 9 5950X, Linux 7.2.10. At radius 6 an idle sparse tick costs 27 µs of device time (9 activity, 18 material), a fully active one 280 µs sparse or dense (0.35 ns per cell), dense idle 86 µs; encode 47–62 µs; round trip 0.26–0.51 ms one tick in flight; 110–142 process allocations per tick inside wgpu; nothing created or waited on: [performance budgets](../../../reference/performance-budgets.md#material-phases-03), with the planned resources per radius (`25.125 N + 40 C + 64` terrain bytes) and the re-measured 02 transfer path (command pass 31 µs, tick 74 µs). No hot spot needed rework. Per-case JSON in `target/evidence/final-03-cells/gpu-cells/` on that machine only. Headless physics only: no native-client frame time and no speedup claim. |
| Acceptance | A04 and A05 satisfied on the RTX 5070 Ti: the 10 simulation and 5 cell intentions, exact transfers and donor conflicts, frozen, mixed and full receivers, ceiling quench, keyed fire and growth, classified sinks and sources; dense/sparse agreement, sleeping receivers and the two-cell halo, climate, rule, edit and chunk reactivation, empty and full activity with the dispatch split, negative keys, seams, rebasing, rollover, presentation rates and catch-up, the unmatched-transfer specimen. A06–A15 not run. |
| Data safety | No case opens a store; every world is generated or synthetic in memory. `world_data/save.redb` was neither opened nor changed (8,237,617,152 bytes, modified 2026-10-11 14:24 local time). |
| Blockers | None for 04. |
| Next action / stop | Stopped at the end of 03. Assign 04. |

### Carried into later stages (03)

1. **Bodies on the material phases** (04): body substeps go after `commit`; contact deformation
   and displacement must go through the same accepted-amount transfers and accounting, and wake
   what they change through the edited bitmap or an equivalent writer.
2. **Rule seed from world identity** (05/06): `GpuWorldConfig::rule_seed` defaults to 0 (the
   baseline hashes); the composition root and the checkpoint must choose and record it.
3. **Checkpoints and activity** (05): a capture holds the end of a tick; resuming needs the tick
   (`first_tick`), the rules, the climate and the seed. Activity is not checkpointed: a resumed
   world wakes its chunks as they activate.
4. **Statistics display** (06): totals come from `MeasureTotals`, not a cell download.
5. **The baseline engine** (06) still loses liquid when magma reaches a coolant at the ceiling
   (its liquid step ignores the accepted amount); the GPU path does not. Removed with the engine.
6. **Larger-window timing** (07): 03 measured device time at radius 6 only; the scale and
   capacity profiles run against the GPU runtime in 04–07.

## 02 boundary (closed)

02 completed on 2026-10-13; its record follows.

## 02 record

| Field | Record |
| --- | --- |
| Assigned stage / checkpoint | 02 — shared GPU runtime and bounded transfer lifecycle. A–D complete. |
| Source state | `experimental`: `1440bf5` (world-runtime and model additions), `6d334a0` (gpu-world, evidence integration, the `transfers` suite), then the documentation commit and this record. The tree was clean before 02; no unrelated work was present. |
| Actual adoption | **A:** `world-runtime` (identities, the `PhysicalWorld` port, receipts, ledger, clock, standalone `bevy_ecs` runtime) and `gpu-world` `context`/`plan`/`resources`: adapter selection by the stable identifier (which `inspect-adapter` now takes from `gpu-world`), limits raised from WebGPU defaults only to the radius-32 plan's needs, error-scoped counted allocation, one timeline (≤ 3 in flight), loss and uncaptured-error health, pipelines compiled once per session. **B:** `layout` (records declared once, WGSL generated; cell constants from `WorldCell::LAYOUT`), `shader` (assembly with line map, naga validation), ordered `apply_edit`/`query_column` kernels over resident slots, identity-checked decoding. **C:** `staging` (four pools, generation-checked map mailbox), `slots` (slot lifecycle, window map), resize with device-to-device migration, the ledger as the CPU consumer, fault injection, stop/drain. **D:** `cargo evidence test gpu --suite transfers` (28 cases, one `gpu-suite` process each, registry-checked), `static.shaders`, per-package dependency rules, pass timestamps. Superseded: evidence's own adapter enumeration (now `gpu-world`'s), the "02 requests a larger binding or splits the array" open choice (a larger binding is requested; otherwise refused). |
| Package decisions | `world-runtime` under `crates/`, `gpu-world` under `adapters/` ([ADR 0002](../../../decisions/0002-native-composition.md#02-package-check)). GPU contracts are not cargo tests (hosted CI has no GPU): the suite lives in the evidence leaf and drives the public APIs; layout probes assemble from `gpu-world`'s exported declarations and run through `submit_external`. `world-store` and `world-render` are not created. |
| Physical limits | Unchanged from 01. New capacities with their source: 256 commands and 64 cosmetic events per tick, 32 chunk-upload and 32 capture buffers, 96 slot changes per tick ([GPU contract](../../../reference/gpu-contract.md#observations-and-reliability)). Radius 32 needs a 138,444,800-byte storage binding; under WebGPU defaults it is refused and radius 31 runs. |
| Decisions / deviations | Edits take a centre cell (the ray is 04's) and wake nothing (activity is 03's). Body, attract/lift and temperature commands are refused as `NotSupported` until 03–04. The only cosmetic producer is fault injection until 04. Uploads activate before a tick's commands and captures copy after them. A retired slot's generation advances at retirement. wgpu 29.0.4 panics from `Device::poll` on a backend-reported loss, so the poll is isolated with `catch_unwind` and turned into the lost state. Dependency rules use `cargo tree -p` because workspace metadata unifies the Bevy baseline's features into `bevy_ecs`. The process runner gained step timeouts. |
| Checks | At `f740577`, clean, 2026-10-13 08:51–08:52 UTC, with `CARGO_BUILD_JOBS=4`: `cargo evidence check docs` passed (150 local links in 50 files, 7 packages with READMEs); `cargo evidence check static` passed (all 12 items, including `static.shaders` for the 2 assemblies and the per-package dependency rules; Clippy 7 diagnostics, all covered by the 3 recorded exceptions); `cargo evidence test cpu` passed (147 tests: 96 registered CPU behaviors, 28 of them baseline, and 51 runner tests); `cargo evidence test gpu --suite transfers --adapter vulkan-10de-2c05` passed (29 items: the registry check and all 28 cases); `cargo evidence test gpu --suite cells --adapter vulkan-10de-2c05` failed as intended (unregistered until 03). Reports in `target/evidence/final-02-{check-docs,check-static,test-cpu,transfers,cells-unregistered}/`. Each intermediate commit was tested before it was made: `1440bf5`'s exact tree in a scratch copy (formatting, Clippy, `test cpu` with its 71-entry registry; only the runner test that needs a git commit identity failed there, as the copy had none) and `6d334a0` with all four lanes. |
| Hardware / measurements | RTX 5070 Ti (`vulkan-10de-2c05`, NVIDIA 615.71.09), Ryzen 9 5950X, Linux 7.2.8. Granted: storage binding 138,444,800 B, buffer 256 MiB, `TIMESTAMP_QUERY` when requested (period 1 ns); the adapter offers 2,147,483,644 B and 1 TiB. Measured transfer lifecycle, resize overlap and planned allocations against the envelopes: [performance budgets](../../../reference/performance-budgets.md#measured-transfer-lifecycle-02). Per-case JSON in `target/evidence/<run>/gpu-transfers/` on that machine only. No application performance claim: no physics runs on the GPU yet. |
| Acceptance | A02 satisfied: production shader roundtrip and golden words, partial dispatch, default-limit and allocation refusal, successful and refused resize without loss, one context. A03 satisfied: real CPU consumer (runtime ledger), delayed and reordered deliveries, held completions, full pools, stale session and slot, same-ID retry and conflict, no frame waits, executed on the GPU. A04–A15 not run. |
| Data safety | No case opens a store; worlds are generated or synthetic in memory. `world_data/save.redb` was neither opened nor changed (8,237,617,152 bytes, modified 2026-10-11 14:24 local time, as at the end of 01). |
| Blockers | None for 03. |
| Next action / stop | Stopped at the end of 02. Assign 03. |

### Carried into later stages (02)

1. **Edits wake nothing** (03): `apply_edit` changes cells without activity metadata; 03 adds
   reactivation to the same command path.
2. **Tool sources and sinks** (03): the sand brush keeps the baseline rule that compacts sand into
   sandstone only up to the elevation ceiling, so sand beyond it is lost, like the quench defect;
   03's accounting classifies tool sources and sinks.
3. **Brush ray, body commands and cosmetic producers** (04): edits target a centre cell; spawn,
   control, attract/lift are refused; contacts and immersion become the ledger's first producers.
4. **Residency policy and revisions** (05): slots, uploads, captures, retirement and resize exist
   as mechanisms; focus movement, dirty revisions, retained snapshots and the store do not. GPU
   edits do not advance a chunk's revision yet.
5. **A real driver loss is untested**: loss is injected with `Device::destroy`; the
   `catch_unwind` path for a backend-reported loss has no specimen on this machine.
6. **Software adapters** are refused without consent; no software adapter exists here, so that
   refusal is not executed.
7. **Per-tick allocations inside wgpu** (07): about 103 per tick on the submit path; application
   pools allocate nothing after warm-up.

## 01 boundary (closed)

01 completed on 2026-10-12; its record follows.

## 01 record

| Field | Record |
| --- | --- |
| Assigned stage / checkpoint | 01 — baseline, contracts and seams. A–D complete. |
| Source state | `experimental`, last tested commit `fa38fdd`, clean tree; the commit after it fills in this record only. No unrelated work was present; a pre-existing GitHub Desktop stash was left untouched. |
| Actual adoption | **A:** `eea8652` pins Rust 1.99.0 and tracks `Cargo.lock`; `58e9648` captures old-writer fixtures. **B:** `tools/evidence` (`monarchy-evidence`) with the `cargo evidence` alias; docs/static/CPU lanes; adapter inspection; GPU suites and `run` fail explicitly; 18 frozen scenario files; the `legacy-baseline` driver; work counters in the engine, renderer and persistence; `coverage.toml` maps all 28 baseline tests; `lint-baseline.toml` holds 3 exceptions. The measured baseline is `e8d49db`. The visual reference (three frames from the qualification scenarios, `tools/evidence/baselines/legacy-visual/`) and the presentation and tuning defaults ([acceptance](acceptance.md#baseline-presentation-and-tuning-reference-01)) record the rendering and tool surface. **C:** `7a84a80` adds `world-model`, migrates every consumer and deletes the duplicates; `legacy_format` keeps the old row bytes. **D:** `0c166e3` adds `deny.toml`, `cargo deny` and `cargo machete` at pinned versions, the CI workflow and its pin check, and the package-naming check. |
| Package decisions | `world-model` exists because the engine, the application, spatial-lib and the runner all consume it. `world-runtime`, `gpu-world`, `world-store` and `world-render` are not created: no consumer exists before 02, and empty forwarding crates are not allowed. `spatial-lib` keeps only the toroidal grid, the chunk manager and the redb backend; its key math moved to `world_model::spatial`. Commands and observations are used by the evidence driver; the runtime ports are recorded as signatures in [GPU contract](../../../reference/gpu-contract.md#runtime-ports-implemented-in-02) for 02 to create. |
| Physical limits | Fixed in [GPU contract](../../../reference/gpu-contract.md#supported-physical-envelope-fixed-in-01) and owned by `world_model::{config, body, units}`: tick 10–240 Hz, catch-up 1–16, body substep ≤ 1/120 s dividing the tick, elevation scale 0.01–5.0 as physical configuration, body radius 2–8, relative density 0.01–25, mass 0.01–1000, speed ≤ 600 world units/s, ≤ 4,096 bodies, brush radius 0–64 and strength 1–255, active radius 1–32. Tolerances are the baseline tests' own, per test in the registry. |
| Decisions / deviations | The elevation scale is physical configuration ([world format](../../../reference/world-format.md#existing-packed-cell)); the acceptance conflict is closed. The run protocol counts a 1,800-tick window instead of 1,800 frames; the calibration and its reason are recorded in the [performance budgets](../../../reference/performance-budgets.md#regression-and-improvement-policy) before any candidate. Measurement windows update continuously when unfocused, because Bevy's default throttles them to 60 Hz. The quench ceiling defect is kept and documented for 03. `cargo deny` found advisories after the baseline was measured: `0e948bb` updates six locked crates at patch level (generator output unchanged), and `deny.toml` accepts four advisories without a compatible fix, each with its reason and stage. `0e948bb` also took the staged deletion of `runtime/hooks.rs` without its module declaration, so that commit alone does not build; `ecd9212` completes the removal. The first full baseline at `79e7ead` was rejected (compositor pacing) and is kept locally as `target/evidence/rejected-baseline-*-79e7ead`. |
| Checks | At `fa38fdd`, clean, 2026-10-12 20:01–20:10 UTC, with `CARGO_BUILD_JOBS=4`: `cargo evidence check docs` passed (137 local links in 48 files); `cargo evidence check static` passed (all 11 items, including `cargo deny`, `cargo machete` and the CI pin check; 7 Clippy diagnostics, all covered by the 3 recorded exceptions); `cargo evidence test cpu` passed (98 tests: all 49 registered behaviors, 28 of them baseline, and 49 runner tests). Reports in `target/evidence/final-{docs,static,cpu}/`. Baseline: `cargo evidence bench --profile qualification --adapter vulkan-10de-2c05 --output target/evidence/baseline-qualification`, the same for `scale`; both passed at `6e108d6`. Drift after the extraction: `cargo evidence bench --profile qualification --adapter vulkan-10de-2c05 --case flow --case body-contact --case coupled --output target/evidence/drift-7a84a80` at `7a84a80` passed, 5/5 valid runs per case, none contended; simulation and body time per tick within 2% of the baseline, frame medians 3–6% lower ([performance budgets](../../../reference/performance-budgets.md#measured-legacy-baseline-01)). |
| Hardware / measurements | Ryzen 9 5950X, RTX 5070 Ti (`vulkan-10de-2c05`, NVIDIA 615.71.09), KDE Wayland, 1920×1080 always-on-top window, no vsync. Summaries in `tools/evidence/baselines/legacy-6e108d6/`; raw samples only in `target/evidence/baseline-{qualification,scale}/` on that machine. Results in the [performance budgets](../../../reference/performance-budgets.md#measured-legacy-baseline-01). The capacity profile is frozen and not run on the baseline. |
| Acceptance | A01 satisfied for 01's scope: source baseline, all 28 test intentions and the tool surface retained; the runner's negative cases pass and a missing suite cannot pass; consumers use the model and the fixtures keep their meaning; the baseline names its hardware and limits; profiles and tolerances are frozen before optimization. A02–A15 not run. |
| Data safety | Every bench and visual-reference run used a new world directory under its run directory and a scratch working directory; since `6e108d6` they are deleted after validation, and the leftovers of earlier pilot runs were removed. The fixture store test works on a copy and removes it. `world_data/save.redb` was never opened by 01. It changed on 2026-10-11 at 14:24 local time (8,074,563,584 → 8,237,617,152 bytes) while no 01 work was running. |
| Blockers | None for 02. |
| Next action / stop | Stopped at the end of 01. Assign 02. |

### Carried into later stages

1. **Measured defects for their owners** (source audit, "Found while measuring"): chunks that stream
   in are never woken (03, A05); the brush cursor marks all four material assets modified every
   frame (06); a brush frame and a tick frame each upload the whole grid (06); the unfocused-window
   policy of the native client (06).
2. **Persistence baseline failures** for 05, unchanged by 01: eviction-only saves without a dirty
   flag, stale reads while a save is pending, an exit flush that omits active and cached chunks, and
   read or decode errors that fall through to generation.
3. **Shader validation** joined `check static` in 02 (`static.shaders`). The legacy Bevy shaders
   under `crates/morbid-app/assets` are validated only at runtime and are removed in 06.
4. **The capacity profile** runs against the GPU runtime (04–07), never against the baseline.

## 00 boundary

**State:** 00 complete (2026-10-10). This was a documentation-only adoption. No runtime code,
manifest, dependency, toolchain or saved-world byte changed. Nothing was built, tested, run on a
GPU or measured, and no acceptance item has passed.

**Baseline:** branch `experimental`, commit `a9c1539` (clean before 00). Source and shaders last
changed in `e1fa1d2`. The supplied archive (`morbid-monarchy-experimental.zip`, SHA-256
`3c677631…871439`) is not in the repository. 00 compared the audit's symbols and its 28 tests with
the checkout instead of comparing digests. The adoption commit is the commit that last changed this
file (`git log -1 -- docs/development/virtual-office/gpu-transition/handoff.md`).

**Confirmed user direction:** GPU physical authority over resident terrain and bodies; CPU owns
intent, residency and I/O; rendering shares the GPU-resident state; the final application keeps
standalone `bevy_ecs` and drops the Bevy engine. This authorizes a transition sprint, not a new game
or physics feature set. The package map, exact algorithms and initial numeric budgets are proposed
engineering choices, not measured outcomes.

**Next action at the end of 00:** assign
[01-baseline-contracts-and-seams.md](01-baseline-contracts-and-seams.md); that has since happened (see above).

## 00 record

| Field | Record |
| --- | --- |
| Assigned stage / checkpoint | 00 — first execution: adopt and reconcile. Complete; no partial checkpoint remains. |
| Source state | `experimental` at `a9c1539` before 00. There was no uncommitted or unrelated work; local untracked state is limited to the ignored `Cargo.lock`, `target/` and `world_data/`, all left untouched. |
| Actual adoption | Architecture now separates the target from a [current baseline](../../../architecture.md#current-implementation) table, and the package map stays proposed. ADR, decision-index and budget states distinguish requested direction, adopted policy and proposed values. The [source audit](source-audit.md) has a checkout reconciliation, corrected rows and dispositions for the retired rule files. The implementation practice gained explicit naming, typed-error, `// SAFETY:` and cold-path rules carried over from those files. [Acceptance](acceptance.md) has a source-reconciled feature inventory, an open conflict and the native-client wording for `resident-world`. Status, the development guide, workflow, world format, roadmap and the office/sprint indexes reflect the checkout. |
| Decisions / deviations | The retired `docs/persona.md`, `docs/rules.md` and `docs/knowledge/rust_knowledge.md` had already been deleted in `a9c1539`. 00 verified the mapping of their intent instead of deleting them. The audit's "four layer entities" was corrected to four per active chunk. Items for 01 are listed below. |
| Checks | See [checks run in 00](#checks-run-in-00). |
| Hardware / measurements | None. No adapter was enumerated; the GPU and performance lanes are not run. |
| Acceptance | A01–A15 not run. |
| Data safety | The user's `world_data/save.redb` (8,074,563,584 bytes, git-ignored) was neither opened nor modified. No application was launched. |
| Blockers | None for 01. |
| Next action / stop | 00 stopped; 01 was then assigned. |

### Deviations requiring 01 attention

1. **Dependency baseline.** An untracked, ignored `Cargo.lock` resolves Bevy 0.19.0, `bevy_egui`
   0.41.0 (egui 0.35.0), wgpu 29.0.4, redb 4.1.0 and bitcode 0.6.9. An offline `--locked`
   metadata check could not finish because some packages were not cached. Treat this lockfile as
   a candidate resolution. Commit it, and remove the ignore, only after a successful `--locked`
   build. The active toolchain is rustc 1.99.0 stable with no pin.
2. **Save safety.** The baseline opens or creates `world_data/save.redb` relative to the working
   directory at startup. Launch the baseline only from a scratch working directory, and capture
   old-writer fixtures into temporary stores (see the
   [development guide](../../../guides/development.md#the-application)).
3. **Persistence baseline failures** that 01 records but does not fix: chunks are saved only on
   LRU eviction, without a dirty flag; a chunk reloaded while its save is pending reads stale or
   absent data; the exit flush leaves out active and cached chunks and in-flight saves. These
   are the fixtures behind A08 and A09.
4. **CPU live-grid consumers** to include in the feature inventory and in command/observation
   design: brush ray march, preview, spawn height and attract/lift targets; brush edits; observer
   walking floor samples; full-grid statistics sums. The residency focus freezes while the
   observer is detached.
5. **Elevation-scale conflict.** Decide its meaning in Checkpoint C (decided in 01: physical
   configuration; see [acceptance](acceptance.md#resolved-conflicts-with-existing-behavior)).
6. **Clock limits.** The live tick-rate (10–240 Hz) and catch-up (1–16) controls need a supported
   range, plus a rule for how body substeps divide a tick.
7. **Coordinates.** Fixture the axis mapping recorded in
   [world format](../../../reference/world-format.md#existing-packed-cell), including the
   misleading `to_ivec2` comment and how nonzero-`z` keys are handled.
8. **Renderer baseline.** Count draws and entities as part of the baseline (four per active chunk;
   676 at radius 6). The UI permits radius 32, which exceeds a default 128 MiB storage binding.
9. **Test discovery.** `spatial-lib` has zero tests. The registry should expect exactly the 28
   `monarch-engine` tests. `runtime/hooks.rs` contains only comments; remove it or replace it with
   a real consumer.

### Checks run in 00

These were run on the final staged adoption diff. There is no repository docs checker yet (01
delivers `cargo evidence check docs`), so 00 used a temporary script outside the repository.

| Check | Result |
| --- | --- |
| Local Markdown link and heading-anchor check over every `*.md` (temporary Python script, run with `python3 -I`) | Passed: 88 local links in 41 files, 0 failures. Baseline before editing: 71 links, 0 failures. In a negative test, the script rejected a missing file and a missing anchor. |
| `git diff --cached --check`, plus a trailing-whitespace scan of `docs/` and `AGENTS.md` | Passed: no errors. |
| Scope | Every staged path is under `docs/`. The SHA-256 of the local `Cargo.lock` and the size/mtime of `world_data/save.redb` are unchanged. |
| Source reconciliation | Inspection only: the 28 `#[test]` names and every audited symbol were located in `a9c1539`. |
| `cargo metadata --locked --offline` | Exited 101: an uncached locked package (`accesskit_consumer` 0.35.0) could not be fetched offline. This is not a build. |
| Not run | `cargo build`, `test`, `fmt`, `clippy` and `doc`; `cargo evidence` (it does not exist yet); any application launch or GPU execution. |

## Record at each stage boundary

Use the fields below with actual values after execution. “Not run,” “unavailable” and “none” are
valid when accurate; they are not a successful qualification. Do not invent commits or reviewers.

| Field | Record |
| --- | --- |
| Assigned stage / current checkpoint | Exact prompt and last completed checkpoint; do not advance because the next file exists. |
| Source state | Branch, last tested commit, uncommitted changes and unrelated work preserved. |
| Actual adoption | Owner/API/data path now canonical; consumers updated and superseded path removed. |
| Decisions / deviations | New evidence and its consequence; canonical doc/ADR updated, including any changed rule or support limit. |
| Checks | Exact commands, selected tests/counts, outcome and tested source/configuration identity. |
| Hardware / measurements | Actual adapter/backend, effective profile and retained report path; distinguish estimates from measurements. |
| Acceptance | Applicable A-items satisfied, failed or unqualified, with evidence location. |
| Data safety | Temporary store/fixture use, outstanding snapshot/save obligations and recovery implications. |
| Blockers | Concrete failure or missing external capability; owner and last tested state, not a speculative concern. |
| Next action / stop | Exact resumable task inside the same stage, or next prompt requiring a new assignment. |

For 07 closure, preserve the final accepted evidence and lasting coverage in their maintained owners
before removing this temporary handoff/sprint. Keep it active when any required gate remains open.

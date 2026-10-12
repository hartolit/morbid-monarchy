# Supplied-source audit and preservation map

**Evidence class: source inspection, plus 01's measurements.** Prepared on 2026-10-10 from the
uploaded archives, then reconciled against the working checkout by prompt 00 on the same date.
Neither review built, tested or ran anything. Paths/symbols below identify the reviewed source at
`a9c1539`. 01 moved some of it (cell, materials, generation and chunk keys now live in
`crates/world-model`), recorded what it found while measuring, and noted its decisions on the
rows below; the [performance budgets](../../../reference/performance-budgets.md#measured-legacy-baseline-01)
own the measurements.

## Provenance

| Input | Identity |
| --- | --- |
| `morbid-monarchy-experimental.zip` | SHA-256 `3c677631e2419d7fd92afbd99818fba5fe18b8a1ba00ce12f8980d44ca871439` |
| `docs(2).zip` | SHA-256 `2f99afd0b6c45d8f9b92fccc74f59c90c2134a590b6d7f2601125367d0ff2f26` |
| Working checkout reconciled in 00 | Branch `experimental`, commit `a9c1539`, clean tree. Source and shaders last changed in `e1fa1d2`; `a9c1539` changed only `AGENTS.md` and `docs/`. |

The archives are not in the repository, so their digests cannot be compared with the checkout.
00 instead re-checked every symbol and test named below against `a9c1539`, and all of them are
present. 00 made one correction: the renderer has four layer entities per active chunk, not four
in total. It also extended the render-sync, chunk-manager and exit-flush rows. It added the rows
for stale reload, database open, CPU live-grid readers, focus attachment, elevation scale, clock
controls, axis mapping and `hooks.rs`.

The workspace contains `monarch-engine`, `morbid-app` and `spatial-lib`; its manifest requests
Bevy 0.19 and `bevy_egui` 0.41.0. The checkout tracks no lockfile, toolchain pin, `.cargo/` or CI
configuration, and `.gitignore` ignores `Cargo.lock`. An untracked local `Cargo.lock` resolves
Bevy 0.19.0, `bevy_egui` 0.41.0 (egui 0.35.0), wgpu 29.0.4, redb 4.1.0 and bitcode 0.6.9. That is
untested local state, not a pin. `cargo metadata --locked --offline` stopped because some locked
packages were not in the local cache, so whether the lockfile is consistent with the manifests
has not been checked. The active toolchain is stable rustc/cargo 1.99.0. 01 resolves and pins a
real supported baseline.

## Source observations and destination

Paths beginning `core/` below are under `crates/monarch-engine/src/`; `runtime/` paths are under
`crates/morbid-app/src/`. References use symbols rather than pretending source line numbers remain
stable throughout extraction.

| Source and observed behavior | Transition responsibility |
| --- | --- |
| `core/world/cell.rs`, `WorldCell(pub u64)`: tightly packed column fields and helpers; maximum elevation 4095, fluid 511, granular 15. | 01 preserves bytes/units independently; 02 tests two-word GPU ABI; 03 retains/corrects physical accounting deliberately. |
| `core/world/chunk.rs`: 64×64-cell chunks. `core/world.rs`: initial active radius 6 per axis. | Preserve geometry in baseline fixtures and explicit resource formulas. |
| `core/world/grid.rs`, `swap_buffers`: swaps then copies the entire cell slice; wake buffers also swap/clear. | 03 establishes valid untouched output without recurring full-cell copy; cannot just delete the copy from partial ping-pong. |
| `core/simulation.rs`, `run_simulation_tick`: full traversal skips sleeping cell rules, then unconditionally sets `cells_dirty`. | 03 separates correct sparse scheduling from physical changes and actual renderer consumption. |
| `runtime/render.rs`, `sync_grid_rendering`: whenever `cells_dirty` is set (every frame that ran a tick, and after edits) the whole grid is copied into `ShaderBuffer.data` for Bevy to upload. All four material layers share that buffer. | 06 removes whole-grid CPU upload. This is one shared update, not four full uploads. |
| `crates/spatial-lib/src/chunk/manager.rs`, active chunks and window movement: retained chunk payloads coexist with the grid; outbound extraction is synchronous CPU access. Chunks leaving the active window move into an LRU cache (`CACHE_CHUNK_SIZE`, 1,152 chunks, computed from the default radii) and are queued for save only when evicted from it. There is no dirty flag, so unmodified generated chunks are saved as well. | 05 replaces payload ownership with residency metadata, ordered GPU capture, generations and bounded retained snapshots. Dirty means changed since the last acknowledged durable or captured revision. |
| `core/simulation/liquid_sim.rs` and `granular_sim.rs`: receiver/donor choices repeatedly reconstruct neighbours' preferences and transfer quantities. | 03 compares proposed/accepted/committed work with the current behavior; speed benefit remains to be measured. |
| `core/world/cell.rs`, `quench`: drains magma, then calls capacity-limited `raise_bedrock` without retaining the unconverted remainder. | 03 adds a conservative ceiling rule/fixture. Ordinary low-elevation tests do not cover this capacity boundary. |
| `core/simulation.rs`: fixed terrain ticks, but thread-seeded `SmallRng` for stochastic rules. | 03 retains cadence, makes keyed randomness explicit and documents changed reproducibility. |
| `core/entities/spherical.rs`, `simulate_rigid_sphere_kinematics`: integrates frame delta through a capped number of substeps; `resolve_sphere_pairs` visits all combinations. | 04 moves bodies to the shared physical clock and complete bounded spatial candidates; source timing/candidate evidence is preserved in 01. |
| `core.rs`: terrain/body/observer Update systems are listed without an explicit semantic chain. | 03–04 define physical pass order; mutual resource exclusion alone is not the intended schedule contract. |
| `core/physics/grid_physics.rs` and `core/entities/spherical.rs`: heightfield triangle contacts, buoyancy, sand ruts/craters and liquid displacement directly mutate the CPU grid. | 04 retains the column-based geometry and test intentions while resolving shared GPU writes collectively. |
| `crates/morbid-app/assets/shaders/landscape/common.wgsl` and `runtime/render.rs`: Bevy PBR shader imports (mesh/view bindings, directional shadow fetch, fog); heightfield-marched terrain shadows and ambient occlusion that resample terrain. Four layer entities are spawned **per active chunk** (676 at radius 6), each with `NoFrustumCulling` and one shared chunk mesh. | 06 replaces engine plumbing and measures derived visibility/lighting alternatives. No current measured bottleneck ranking is implied. |
| `runtime/persistence.rs`, `handle_load_requests`: errors/decoding failures become `None`, then trigger terrain generation. | 05 separates absent from corrupt/unavailable reads and preserves existing data. |
| `runtime/persistence.rs`, `handle_load_requests`: a load does not check `ChunkSaveQueue` or in-flight `ChunkSaveTask`s. A chunk that is evicted and then requested again before its save commits is read stale or absent (and then generated), and the stale copy can later overwrite the newer save. This is inferred from source; it has not been observed in a run. | 01 records it as baseline behavior in a temporary-store fixture; 05 makes the newest retained revision win (A08). |
| `runtime/persistence.rs`, `process_save_queue`: removes queued payloads, saves in `Task<()>`, logs write failure without returning retry state. | 05 retains snapshot/payload obligations until durable acknowledgement. |
| `runtime/persistence.rs`, `emergency_flush_on_exit`: flushes only the save queue. Active chunks, LRU-cached chunks and in-flight save tasks are not captured, yet it logs "Safe to terminate". | 05 implements complete checkpoint and close ownership; distinguish orderly save from crash recovery. |
| `runtime/persistence.rs`, `initialize_database`: at startup, before any window, `Database::create` opens or creates `world_data/save.redb` relative to the working directory, and panics on failure. | Experiments use a scratch working directory and temporary stores. 05–06 add explicit world selection and typed open errors. |
| `crates/spatial-lib/src/chunk/storage.rs`: redb `spatial_chunks` table keyed by three signed coordinates, with app-encoded chunk bytes. Startup uses seed 69. | 01 captures old-writer fixtures; 05 versions semantic data and imports with explicit missing-metadata interpretation. |
| `core/simulation.rs`, `SimulationEventQueue`: unbounded channel exists; fire/biology local-event arguments are unused and `runtime/hooks.rs` consumer is commented out. | 02/04 deliver actual bounded outcomes and a CPU consumer. Do not claim the baseline currently floods the queue or has implemented gameplay events. |
| `runtime/input.rs`, `dev_tools.rs`, renderer/UI: brushes, body presets/control, camera, attract/lift, climate/lighting and grid resize. | 01 freezes the real feature inventory; 06 proves native tool behavior, not just menu presence. |
| CPU reads of the live grid outside the solvers: `runtime/dev_tools/brush.rs` `raymarch_grid` (cursor preview, edit target, sphere spawn height, attract/lift target) and `handle_brush_input` (`set_cell`/`wake_cell`). `runtime/dev_tools/ui.rs` sums fluid volume and elevation over every cell on each frame the statistics panel is shown. `core/entities/observer.rs` samples floor height every frame in walking (non-noclip) mode. | 04 provides GPU ray/query/edit commands and observer ground queries (or a GPU-side observer rule); 06 computes the statistics as GPU reductions. None of these may use a full-grid fetch. |
| `runtime/input.rs`, `sync_world_focus`: the residency focus follows the observer only while it is grid-attached (toggled with G), so detaching freezes streaming in place. | Preserve as a supported control in 05–06, or record an explicit scope change. |
| `runtime/dev_tools/ui.rs`: the elevation-scale control writes `GlobalPhysicsConfig::elevation_scale` (0.01–5.0) at runtime. Body contact, spawn height and rendering all use it, and it is not saved. | Decided in 01: physical configuration, applied at a tick boundary and checkpointed; see [acceptance](acceptance.md#resolved-conflicts-with-existing-behavior). |
| `runtime/dev_tools/ui.rs`: tick rate 10–240 Hz and catch-up 1–16 ticks/frame are live controls; body substeps (≤1/120 s, at most 8 per frame) ignore them. | 01 set the supported ranges and the rule (substeps of at most 1/120 s divide the tick; [GPU contract](../../../reference/gpu-contract.md#supported-physical-envelope-fixed-in-01)); 04 implements it. |
| Axis mapping: `sync_world_focus` and `ChunkKey::from_world`/`to_ivec2`. The mapping is recorded in [world format](../../../reference/world-format.md#existing-packed-cell). | Captured in 01's old-writer fixtures, including a synthetic `z = 1` row; the conversions are named in `world_model::spatial`. 05 import decides how to handle rows with nonzero `z`. |
| `runtime/hooks.rs`: a compiled module that contains only comments. | Removed in 01. 02/04 add the real consumer with the bounded outcomes. |

### Found while measuring (01)

| Source and observed behavior | Transition responsibility |
| --- | --- |
| `ToroidalGrid::load_chunk` writes a chunk's cells without scheduling them; only grid creation and resize wake every cell. A chunk that streams in after startup stays asleep until a neighbour or an edit wakes it (streaming runs measured 0 awake cells). Generated water is level, so the visible effect is that growth and other rules do not start in newly loaded chunks. | 03 wakes ingress chunks and their boundary dependants; A05 covers it. Record the change of behavior. |
| `update_brush_cursor` iterates all four landscape materials with `Assets::iter_mut` every frame, even with no brush selected. Bevy queues `AssetEvent::Modified` for each, so every material is prepared again each frame. | 06 keeps cursor state out of material assets. Baseline frame cost includes it. |
| A material brush marks the grid dirty in addition to the tick that follows, so a frame with an edit and a frame with a tick each upload the whole grid. The flow case measured 0.54 GiB/s of grid uploads against 0.31 GiB/s of tick copies (1.7×). | 06 removes whole-grid uploads; edits reach the GPU as commands. |
| The mixed scenario's lava brush (radius 3, strength 6, every 2 ticks) pours into the lake that the climate swing freezes; by tick 1300 a column stands far above the surrounding ice ([visual reference](../../../../tools/evidence/baselines/legacy-visual/README.md)). Inference from the rules, not a cell readout: `inject_fluid` lets injected magma take a column over, and the quench rule turns resident magma into stone bedrock of the same height, so each stroke raises the bedrock instead of spreading. The column climbs towards the 4,095 elevation ceiling, where the documented quench defect deletes magma. The frozen mixed case therefore exercises the ceiling during measurement. | 03 decides how injected magma spreads and quenches, and corrects the ceiling accounting; it records the change of behavior and keeps this case. |
| Bevy's default `WinitSettings::game()` updates an unfocused window at 60 Hz. Measurement windows override it to update continuously; the interactive application keeps the default. | 06 chooses the native client's unfocused policy explicitly. |

### Calculated scale, not measured traffic

For radius r on both axes, cell count is `N = (2*r + 1)^2 * 64^2`, and one packed payload is `8*N`
bytes. Radius 6 gives 692,224 cells / 5.28125 MiB; radius 16 gives 4,460,544 / 34.03125 MiB; radius 32
gives 17,305,600 / 132.03125 MiB. At 60 dirty uploads/s those payloads alone are approximately
0.30945, 1.99402 and 7.73621 GiB/s. Dirty-render cadence is not guaranteed to equal physical-tick
cadence; these values are conditional arithmetic, not bus measurements.

Front/back grids, retained active chunks and render-asset bytes represent roughly four packed
payload copies before wake arrays, cache, transient staging and allocator overhead. At radius 32
that is about 528.125 MiB of CPU cell payloads. 01 measured the retained cell payload at
30.9 MiB for radius 6 and 163.1 MiB for radius 16 (182.4 MiB while streaming). Four materials
sharing a buffer do not imply four separate grid uploads.

## Documentation adaptation

The supplied reusable docs are for a workflow/orchestration application. Retain their useful
management structure: concise agent entry point, canonical vision/architecture/status/roadmap,
implementation/documentation practices, executable evidence, decisions and a temporary virtual
office with whiteboard, sequential assignments, handoffs and closure cleanup.

Do not import that project's daemons, providers, capability authorities, workflows, browser/editor
protocols, distributed command replay or historical decision list. GPU command receipts here are
bounded session mechanics, not a durable workflow event log. The rewritten practices add physical
units/invariants, pass/data ownership, device limits, shader ABI/lifetimes, numerical error,
resource/latency budgets, fair measurement and storage consistency because this project needs them.

The original game docs also needed reconciliation. The original `docs/architecture.md` promoted a
crate for every feature, and `docs/knowledge/rust_knowledge.md` forbade manual SoA in favour of ECS
and included blanket low-level prescriptions. Preserve modular intent, complete implementations and
readable code, but replace unmeasured rules with explicit ownership and evidence. `const fn` permits
const evaluation; it does not make every call a precomputed value. Blanket `inline(always)` is not
a performance gate.

### Retired rule files

`docs/persona.md`, `docs/rules.md` and `docs/knowledge/rust_knowledge.md` were deleted in
`a9c1539`, before 00 ran. 00 checked where each of their useful rules now lives. No
`docs/knowledge/` directory remains, and no active link refers to these files.

| Retired rule | Disposition |
| --- | --- |
| Plain technical language; argue with evidence rather than agree | `AGENTS.md` (challenge a design with an alternative and a discriminating test; separate source, evidence, inference and choice); documentation practice, *Evidence language*. |
| Load architecture and rules before acting | `AGENTS.md`, *Read before changing the repository*. |
| Rust 1.96.1 stable / 1.98.0 nightly | Dropped as never-tested pins; 01 commits a tested `rust-toolchain.toml`. |
| Complete compile-ready code; no placeholders, mocks or TODO bypasses | `AGENTS.md` completion rule; implementation practice. |
| No magic numbers | Implementation practice, *Configuration, units and bounds*: a named source, unit and range for each bound, without a switch for every literal. |
| Why/how comments; no changelog or conversational comments; semantic naming | Documentation practice, *Comments*; implementation practice (00 added an explicit naming rule). |
| `unwrap`/`expect` only for a proven invariant; enum errors, not strings | Implementation practice (00 made typed error variants explicit). |
| `unsafe` kept tiny, each with a `// SAFETY:` proof | Implementation practice (00 made the `// SAFETY:` comment explicit). |
| Zero-allocation hot paths; trust iterators/LLVM first; readable cold paths | Performance practice, *Bound hot work*; implementation practice, *Hot paths* (00 added the cold-path rule). |
| Atomics ordering; minimal lock scope | Implementation practice. The preference for lock-free and spin locks was removed. |
| Type-state; no `dyn` inside hot loops | Implementation practice, *Keep interfaces narrow*. |
| ECS manages all SoA | Removed: layout follows access and measured cost. |
| `#[inline(always)]`, `#[cold]`, branch hints and `const fn` as speedups | Removed as blanket rules; they need a measured profile. |
| MMIO/volatile, inline/naked asm, kernel FPU state, `no_std`, ABI register limits | Dropped: bare-metal guidance that nothing in this application uses. |
| One crate per feature (old architecture) | Replaced by `AGENTS.md`, implementation practice and ADR 0002. |

## Existing test inventory

These are the **28 test functions present in `a9c1539`**; `crates/spatial-lib` has none. 01 ran
them (all pass, also in 25 repeated runs) and mapped each to a stable behavior ID, expectation and
destination in [`coverage.toml`](../../../../tools/evidence/coverage.toml), which `cargo evidence
test cpu` enforces.
Names containing “mass” do not independently establish calibrated physical units. Keep original
intent plus independent corrected-limit tests; do not mechanically freeze known bad behavior.

### `crates/monarch-engine/src/core/entities/spherical.rs` — 12 tests

- `closest_point_respects_triangle_regions`
- `submerged_fraction_matches_cap_volume`
- `resting_sphere_settles_into_soft_ground_without_drifting`
- `sphere_rolls_down_a_slope`
- `driven_sphere_crosses_bumpy_ground_without_hopping`
- `full_speed_on_flat_rock_never_leaves_the_ground`
- `rolling_leaves_a_bounded_trail_with_conserved_berms`
- `driven_sphere_climbs_a_hill`
- `hard_landing_craters_and_conserves_sand`
- `light_sphere_floats_and_dense_sphere_sinks`
- `wake_scales_with_how_much_of_the_sphere_cuts_the_surface`
- `plunging_sphere_splashes_without_losing_water`

### `crates/monarch-engine/src/core/physics/grid_physics.rs` — 1 tests

- `apportion_is_exact_and_respects_capacity`

### `crates/monarch-engine/src/core/simulation.rs` — 10 tests

- `dam_break_conserves_liquid_and_settles`
- `walled_basin_fills_level_and_rests`
- `granular_pile_conserves_mass_and_settles`
- `chaotic_terrain_conserves_liquid_and_solid_mass`
- `oil_poured_into_a_lake_stays_a_patch`
- `still_water_freezes_from_the_shore_and_thaws_back`
- `ice_is_a_floor_that_liquid_cannot_enter`
- `magma_touching_water_solidifies_and_boils_it`
- `burning_oil_consumes_its_fuel_and_burns_out`
- `fixed_clock_runs_whole_ticks_and_caps_catch_up`

### `crates/monarch-engine/src/core/world/cell.rs` — 5 tests (in `crates/world-model/src/cell.rs` since 01)

- `small_inflow_is_absorbed_by_a_deeper_pool`
- `larger_inflow_takes_the_column_over_without_losing_volume`
- `magma_and_coolant_quench_into_rock_either_way_round`
- `ice_is_solid_and_phase_changes_keep_volume`
- `draining_dry_clears_the_liquid_state`


## Limits of this audit

No current CPU/GPU bottleneck ranking, speedup, driver support, practical maximum world size or
successful legacy import was established here. No supplied test establishes the new GPU execution,
sparse scheduling, bounded observations or complete checkpoint lifecycle. Those are assigned work,
not existing capabilities. Source-derived defects and risks are scoped into the sprint rather than
copied into a second competing issue register.

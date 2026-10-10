# Supplied-source audit and preservation map

**Evidence class: source inspection only.** Prepared on 2026-10-10 from the uploaded archives. No
Rust build, gameplay execution, GPU capture or performance benchmark was performed for this package.
Paths/symbols below identify the reviewed source; 00 must reconcile a newer checkout before editing.
The archives contained no Git history from which to establish a source commit.

## Provenance

| Input | SHA-256 |
| --- | --- |
| `morbid-monarchy-experimental.zip` | `3c677631e2419d7fd92afbd99818fba5fe18b8a1ba00ce12f8980d44ca871439` |
| `docs(2).zip` | `2f99afd0b6c45d8f9b92fccc74f59c90c2134a590b6d7f2601125367d0ff2f26` |

The source workspace contains `monarch-engine`, `morbid-app` and `spatial-lib`; its manifest requests
Bevy 0.19 and `bevy_egui` 0.41.0. These are source declarations, not verified compatible resolutions.
No tracked lockfile, pinned Rust toolchain or CI configuration was supplied. `.gitignore` explicitly
ignores `Cargo.lock`. The rewritten docs require 01 to resolve/pin a real supported baseline.

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
| `runtime/render.rs`, `sync_grid_rendering`: dirty grid is copied into `ShaderBuffer.data`. All four material layers share that buffer. | 06 removes whole-grid CPU upload. This is one shared update, not four full uploads. |
| `crates/spatial-lib/src/chunk/manager.rs`, active chunks and window movement: retained chunk payloads coexist with the grid; outbound extraction is synchronous CPU access. | 05 replaces payload ownership with residency metadata, ordered GPU capture, generations and bounded retained snapshots. |
| `core/simulation/liquid_sim.rs` and `granular_sim.rs`: receiver/donor choices repeatedly reconstruct neighbours' preferences and transfer quantities. | 03 compares proposed/accepted/committed work with the current behavior; speed benefit remains to be measured. |
| `core/world/cell.rs`, `quench`: drains magma, then calls capacity-limited `raise_bedrock` without retaining the unconverted remainder. | 03 adds a conservative ceiling rule/fixture. Ordinary low-elevation tests do not cover this capacity boundary. |
| `core/simulation.rs`: fixed terrain ticks, but thread-seeded `SmallRng` for stochastic rules. | 03 retains cadence, makes keyed randomness explicit and documents changed reproducibility. |
| `core/entities/spherical.rs`, `simulate_rigid_sphere_kinematics`: integrates frame delta through a capped number of substeps; `resolve_sphere_pairs` visits all combinations. | 04 moves bodies to the shared physical clock and complete bounded spatial candidates; source timing/candidate evidence is preserved in 01. |
| `core.rs`: terrain/body/observer Update systems are listed without an explicit semantic chain. | 03–04 define physical pass order; mutual resource exclusion alone is not the intended schedule contract. |
| `core/physics/grid_physics.rs` and `core/entities/spherical.rs`: heightfield triangle contacts, buoyancy, sand ruts/craters and liquid displacement directly mutate the CPU grid. | 04 retains the column-based geometry and test intentions while resolving shared GPU writes collectively. |
| `crates/morbid-app/assets/shaders/landscape/common.wgsl` and `runtime/render.rs`: Bevy shader imports, repeated terrain sampling for lighting, four layer entities with `NoFrustumCulling`. | 06 replaces engine plumbing and measures derived visibility/lighting alternatives. No current measured bottleneck ranking is implied. |
| `runtime/persistence.rs`, `handle_load_requests`: errors/decoding failures become `None`, then trigger terrain generation. | 05 separates absent from corrupt/unavailable reads and preserves existing data. |
| `runtime/persistence.rs`, `process_save_queue`: removes queued payloads, saves in `Task<()>`, logs write failure without returning retry state. | 05 retains snapshot/payload obligations until durable acknowledgement. |
| `runtime/persistence.rs`, `emergency_flush_on_exit`: flushes the current queue, not a complete capture of resident state and outstanding save tasks. | 05 implements complete checkpoint and close ownership; distinguish orderly save from crash recovery. |
| `crates/spatial-lib/src/chunk/storage.rs`: redb `spatial_chunks` table keyed by three signed coordinates, with app-encoded chunk bytes. Startup uses seed 69. | 01 captures old-writer fixtures; 05 versions semantic data and imports with explicit missing-metadata interpretation. |
| `core/simulation.rs`, `SimulationEventQueue`: unbounded channel exists; fire/biology local-event arguments are unused and `runtime/hooks.rs` consumer is commented out. | 02/04 deliver actual bounded outcomes and a CPU consumer. Do not claim the baseline currently floods the queue or has implemented gameplay events. |
| `runtime/input.rs`, `dev_tools.rs`, renderer/UI: brushes, body presets/control, camera, attract/lift, climate/lighting and grid resize. | 01 freezes the real feature inventory; 06 proves native tool behavior, not just menu presence. |

### Calculated scale, not measured traffic

For radius r on both axes, cell count is `N = (2*r + 1)^2 * 64^2`, and one packed payload is `8*N`
bytes. Radius 6 gives 692,224 cells / 5.28125 MiB; radius 16 gives 4,460,544 / 34.03125 MiB; radius 32
gives 17,305,600 / 132.03125 MiB. At 60 dirty uploads/s those payloads alone are approximately
0.30945, 1.99402 and 7.73621 GiB/s. Dirty-render cadence is not guaranteed to equal physical-tick
cadence; these values are conditional arithmetic, not bus measurements.

Front/back grids, retained active chunks and render-asset bytes represent roughly four packed
payload copies before wake arrays, cache, transient staging and allocator overhead. At radius 32
that is about 528.125 MiB of CPU cell payloads. Real residency/peak allocations still require 01
measurement. Four materials sharing a buffer do not imply four separate grid uploads.

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

The original game docs also need reconciliation. `docs/architecture.md` promotes a crate for every
feature; `docs/knowledge/rust_knowledge.md` forbids manual SoA in favour of ECS and includes blanket
low-level prescriptions. Preserve modular intent, complete implementations and readable code, but
replace unmeasured rules with explicit ownership and evidence. `const fn` permits const evaluation;
it does not make every call a precomputed value. Blanket `inline(always)` is not a performance gate.
00 retires the superseded rule/persona files instead of leaving contradictory instructions active.

## Existing test inventory

These **28 test functions are present in source, not reported passing**. 01 reads their assertions,
records failed baseline behavior honestly and maps each to a stable behavior ID and destination.
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

### `crates/monarch-engine/src/core/world/cell.rs` — 5 tests

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

# Idea: Chunked Procedural World Domain

## Raw Idea Input
Remove the terrain/tile map logic. I don't like the current state of this.
Ideally I'd want a procedurally generated map, which can be dynamic in nature.

I'm thinking the following to create this idea:
1. Create a world module for isolation  (important)
2. For the generation itself I was thinking of generating in chunks. For the spatial logic I've copy/pasted a snippet from a previous project `spatial` which we could use to draw inspiration from. Though most of the logic is for a spherical world, which doesn't fit this project, so it'll need to be adapted.
I'm not sure how this spatial logic is going to be used yet as I want it to be used for both entity logic and world generation. Should we just use a single spatial index? Or multiple e.g. entity and world spatial indexes?
3. For the procedural generation itself, I was thinking of defining a top level theme enum for a chunk, like:
```rust
pub enum ChunkTheme {
    Dark,
    GrassPlane,
    Cave,
    Ocean,
}
```

Where we define a color pallette for each theme:
```rust
impl ChunkTheme {
    pub fn get_color_pallette(theme: ChunkTheme) -> ColorPallette {
        match theme {
            // Logic for returning a ColorPallette object
        }
    }

    pub fn base_layer(){}
}
```

Then when we generate the ChunkTiles it'll be something like this:
```rust
pub struct ChunkTiles {
    pub theme: ChunkTheme,
    pub proc_assets: Vec<ProcAsset>
}
```

We'll then use our base_layer with proc_assets to generate the rest of the chunk.
```rust
pub struct ProcAsset {
    pub theme: ProcTheme,
    pub intensity: u8,
    pub variant: u8,
    pub position: BoundingBox,
}

pub enum ProcTheme {
    Tree,
    Bush,
    Grass,
    Rock,
}
```

And then create some generation logic to generate different patterns based on the ProcAsset and ChunkTheme.

Ideally we'd only want to generate the chunk once and then store it persistently. That way we can save on cycles and performance not to mention unlocking peristant world interactivity.

There is a few challenges we'll have to think of though. Like:
1. How would interactivity work? How can we efficiently interact with the world/ChunkTiles? Say if we wanted a player walked into a tree, bush or other world objects (in this case defined as ProcTheme/ProcAsset), or a player swimming in the ocean? How would we be able to create a fast dynamic procedural world with this interactivity?
This calls for isolation between world objects and the procedural generation but also a unique link between the two in order to manipulate the generation itself.

## User Goal (Plain Language)
Replace the throwaway hardcoded terrain prototype with a real world-generation direction that is architecturally clean, chunk-based, persistent, and interactive. Success means the project gets a dedicated world domain, a generation model that can produce themed chunks deterministically, and a design that supports fast collision, interaction, persistence, and future dynamic changes without collapsing world data and live entity concerns into one muddled system.

## Context
This touches the current Bevy baseline in `mm-app`, the pure domain boundary in `mm-core`, and a reference `spatial/` module copied from a previous spherical-world project. The current app-side terrain path was a temporary visualization shortcut. The next step is not “make nicer tiles,” but to establish the correct ownership model for chunk coordinates, procedural chunk content, persistent mutations, runtime streaming, and spatial queries for both world content and entities.

## Evidence Inventory
- Available evidence:
  - `doc/architecture.md` requires `mm-core` to remain pure and deterministic, with no filesystem or app bootstrap concerns.
  - `crates/mm-app/src/lib.rs` contained a hardcoded terrain atlas path in `setup_scene` with `spawn_terrain` and `terrain_tile_index`; that path has now been removed so it does not constrain the next design.
  - `mm-core` already uses ECS-native simulation and 3D-aware `Vec3` movement semantics while constraining gameplay motion to the XY plane and preserving Z for indexing/layering.
  - `spatial/types.rs` contains a reusable `ChunkKey { x, y, z }` concept and chunk-view patterns, but most of the traversal logic is spherical-world-specific and not directly usable as-is.
  - `spatial/index.rs` contains a dynamic entity spatial index keyed by `ChunkKey`, with `entity_to_chunk` reverse lookup and chunk-local entity lists.
  - `spatial/components.rs` contains a lightweight `CurrentChunk` component pattern that can inform entity chunk tracking.
  - `spatial/systems.rs` demonstrates a clean sync model: movement updates an entity’s `CurrentChunk`, then a spatial index is updated incrementally.
- Missing evidence required for decision:
  - Desired chunk dimensions in world units and how they map to render scale.
  - Whether the world is infinite, bounded, or region-bounded with streaming edges.
  - Persistence backend preference: file snapshots, embedded DB, save slots, or network-backed persistence.
  - Whether world interactions should mutate tiles, only mutate spawned objects, or both.
  - Whether world objects should always exist as ECS entities or only materialize as ECS entities when near active players.
  - Expected query mix: collision-heavy, AI-heavy, pathfinding-heavy, or generation-heavy.
  - Whether theme transitions must be hard chunk boundaries or blended across chunk borders.

## Constraints (from doc/architecture.md)
- `mm-core` must remain the source of truth for deterministic world/domain logic.
- `mm-app` must remain a thin runtime/application layer and own asset loading, rendering, startup, and persistence I/O.
- No filesystem coupling or runtime-specific persistence code belongs in `mm-core`.
- Avoid magic numbers; world/chunk sizing and generation defaults must be centralized in typed config.
- Organize by semantic boundaries: world generation, world state, and runtime streaming should not be mixed into one file.
- Do not introduce a new crate unless a real boundary demands it.

## Assumptions
- The game remains top-down or near-top-down, with gameplay motion on XY and Z reserved for indexing/layering.
- Chunk generation should be deterministic from world seed plus chunk coordinates.
- Persisted changes should be stored as mutations/deltas over generated chunk baselines rather than storing a fully regenerated world every frame.
- Static world content and dynamic entities have different churn rates and query patterns.
- Not every generated world object needs to be a live ECS entity at all times.
- The first procedural slice can use hard chunk-theme boundaries and a debug-first render path so domain and persistence concerns land before visual polish.
- The first shipped persistence path should remain entirely in `mm-app`, using flat files and `serde`-driven serialization over pure `mm-core` world models.
- The first chunk representation should be a coarse base theme plus deterministic `ProcAsset` stamps rather than a dense tile map.
- Determinism must be strong enough to remain safe for future multiplayer or replay validation: no ambient RNG, no unordered iteration dependence, and no chunk-order dependence.

## Unknowns / Questions
- [x] [high] First slice uses hard chunk-theme boundaries. Cross-chunk blending is deferred until deterministic chunk generation, persistence, and interaction are stable.
- [x] [high] First shipped persistence backend lives in `mm-app` as a flat-file chunk store: `RON` in debug-oriented builds for inspectability, `bitcode` in release-oriented builds for load speed and compactness.
- [x] [high] First interaction slice should prove both mutation and traversal semantics with one blocking object (`Rock`), one destructible object (`Bush`), and one traversable/swimmable surface semantic.
- [x] [medium] Initial active chunk radius targets a 3x3 window around the player (`radius = 1`), with a 5x5 window (`radius = 2`) kept as the first scalability bump if runtime view coverage demands it.
- [x] [medium] First chunk generation emits a single coarse base layer/theme plus `ProcAsset` stamps. Dense tile-like cell generation is deferred until there is a concrete gameplay need for finer terrain resolution.
- [x] [medium] First procedural slice renders with colored rectangles/gizmos rather than sprite-driven terrain so generation, persistence, and interaction logic remain independent from that temporary visualization.
- [x] [low] Commit to multiplayer-safe deterministic generation semantics now: per-chunk seeded RNG, ordered collection handling, and chunk-order-independent generation.

## Goal -> Signal Traceability
| Goal | Required signal | Collection point | Validation threshold |
| --- | --- | --- | --- |
| Remove throwaway terrain path | No hardcoded terrain spawn path remains in `mm-app` | `crates/mm-app/src/lib.rs` | Terrain atlas constants/functions removed |
| Isolate world ownership | Dedicated world domain surface exists in plan | `mm-core` module ownership map | World logic is not smeared into app bootstrap |
| Support deterministic chunk generation | Same seed + chunk key yields same chunk snapshot | future `mm-core::world::generation` tests | Repeated generation is byte-for-byte stable |
| Support interaction without full regeneration | Mutations can be applied over a generated chunk snapshot | future `world` state/mutation tests | Changed objects persist without rerunning whole world generation |
| Keep runtime scalable | Chunk loading/unloading bounded by active view radius | future app/runtime integration | No full-world spawn; only active chunk window is materialized |
| Keep spatial queries fast | Entity queries and world-content queries avoid O(world) scans | future profiling/tests | Queries target chunk/key scopes, not global scans |

## Options Explored
### Option A - Single Unified Spatial Index For Everything
- Architecture fit:
  - Moderate. A single index appears simple, but it conflates two different domains: dynamic entity occupancy and persistent/generated world content.
- Performance risk:
  - Medium to high. Static world data and dynamic entity churn will compete for the same data structure and update pathways.
- Regression vectors:
  - Higher coupling between generation, persistence, collision, streaming, and AI queries.
  - Risk of “god index” behavior with mixed semantics and ad hoc flags.
- Complexity/cost:
  - Low initial setup cost, high long-term maintenance cost.
- Verdict: High Regression Risk

### Option B - Shared Chunk Vocabulary, Separate World Store and Entity Index
- Architecture fit:
  - Strong. Shared coordinate/key types live in the world domain, while dynamic entity indexing stays separate from persistent chunk content.
- Performance risk:
  - Moderate but controllable. Each structure can be optimized for its own workload.
- Regression vectors:
  - Requires careful linking between persistent world objects and runtime entities.
  - Requires discipline so duplicated source-of-truth state does not emerge.
- Complexity/cost:
  - Moderate upfront cost, much better long-term clarity.
- Verdict: Feasible

### Option C - Chunk Store Only, No Dedicated Entity Spatial Index Initially
- Architecture fit:
  - Partial. Simpler than Option B, but punts on a real answer for dynamic entity queries.
- Performance risk:
  - Medium. It may work for a trivial prototype but will degrade quickly once AI, projectiles, or many moving entities appear.
- Regression vectors:
  - Forces a second index retrofit later.
  - Encourages entity/world scans over chunk content structures not designed for dynamic churn.
- Complexity/cost:
  - Lowest short-term cost, high rewrite risk later.
- Verdict: Feasible with Mitigations

## Preferred Direction
Select **Option B**: keep a **shared chunk coordinate vocabulary** and **separate indices/stores**.

The cleanest shape is:
- a pure `world` domain in `mm-core` that owns chunk coordinates, chunk themes, generation inputs/outputs, persistent chunk snapshots, mutation records, and world-object identity;
- a **world chunk store** for generated/persisted chunk content;
- a separate **entity spatial index** for high-churn live ECS entities;
- runtime linking in `mm-app` that materializes only nearby chunk visuals and optionally nearby interactive world objects as ECS entities;
- a flat-file persistence adapter in `mm-app` that serializes pure `mm-core` world state with human-readable `RON` during debug-oriented development and compact `bitcode` for release-oriented builds.

The first chunk payload should stay intentionally light: one coarse base theme/material identity plus deterministic `ProcAsset` stamps carrying stable object ids, bounds, variants, and interaction semantics.

This avoids the “single god index” trap while still reusing one spatial vocabulary. The shared type should be the coordinate/key model, not one giant shared data structure.

## Decision Checkpoint (if 2+ options remain plausible)
- Discriminating experiment:
  - Implement a thin vertical slice with one generated chunk, one persistent mutation, one blocking world object, and one moving player. Measure code-path complexity for (a) collision query, (b) chunk save/load, and (c) entity move update under Option B assumptions.
- Falsification signal for Option A:
  - If the unified index needs object-kind branching, persistence metadata, and dynamic entity churn handling in the same write path, it is too coupled.
- Falsification signal for Option B:
  - If linking generated world objects to runtime entities requires duplicated identity maps or repeated full-chunk scans, the split design needs refinement before broader rollout.

## Delivery Mode
- Selected mode: Phased
- Rationale:
  - This is structurally risky and touches domain modeling, persistence boundaries, runtime streaming, rendering, and interaction. A phased path is safer than forcing a single-pass implementation that risks birthing a monolith.

## Execution Plan (Approval Gate)
- [x] Remove the temporary hardcoded terrain/tile-map rendering path from `mm-app`.
- [x] Create `mm-core::world` as the isolated domain entry point for chunk/world modeling.
- [x] Introduce reusable chunk coordinate/value types adapted from `spatial/` for a planar world, not a spherical world.
- [x] Define deterministic world generation inputs: world seed, chunk size, generation config, theme-selection rules, and a per-chunk seeded RNG strategy with no ambient entropy.
- [x] Define pure chunk output models: a coarse base layer/theme, `ProcAsset` stamps, collision/material semantics, and stable object identifiers.
- [x] Define chunk mutation/state overlays so interactions persist without re-running full generation.
- [x] Decide and implement the split between persistent world store and live entity spatial index.
- [x] Add a thin `mm-app` runtime path that streams/generates a 3x3 active chunk window around the player and renders colored debug chunk/asset visuals.
- [x] Add first interaction semantics for one blocker object (`Rock`), one destructible object (`Bush`), and one traversable/swimmable surface semantic.
- [x] Add persistence integration in `mm-app` for generated chunk snapshots plus deltas, using a flat-file adapter over `serde` world models with `RON` in debug-oriented builds and `bitcode` in release-oriented builds.
- [x] Validate determinism, chunk lookup speed, persistence round-trip, and runtime streaming behavior, including order-independent chunk generation.

## Module Ownership Map
- `crates/mm-core/src/world/mod.rs`: world-domain entry point and public exports.
- `crates/mm-core/src/world/types.rs`: cohesive world-domain model surface containing planar chunk coordinates, chunk-local geometry, coarse material/traversal types, themes, procedural assets, mutation overlays, chunk snapshots/state, and chunk-view helpers.
- `crates/mm-core/src/world/generation.rs`: deterministic chunk generation from seed/config/theme with per-chunk seeded RNG and order-independent output rules.
- `crates/mm-core/src/world/tests.rs` or inline module tests: determinism, mutation, traversal, and chunk-query behavior.
- `crates/mm-app/src/lib.rs`: thin orchestration only; remove hardcoded terrain, register world runtime systems, keep asset/render integration.
- `crates/mm-app/src/world_runtime.rs`: chunk streaming, active-radius management, debug visualization, and render-side chunk entity lifecycle.
- `crates/mm-app/src/world_persistence.rs`: flat-file chunk persistence adapter, format switching, and app-layer save/load orchestration.
- `doc/ideas/2026-03-27-procedural-chunked-world.md`: single source of truth for this idea’s decisions and execution history.

## Feature Flag Plan (Debug/Diagnostics)
- Compile-time flags:
  - No new compile-time diagnostics flags required for the first domain-model slice.
  - Persistence format switching can rely on `cfg(debug_assertions)` or an explicit app-layer feature once the persistence adapter lands.
- Runtime flags:
  - Future optional debug toggle for chunk bounds, chunk ids, and proc-asset overlays.
- Release-default behavior:
  - First procedural slice may still use simple colored debug primitives as the primary renderer, but chunk generation, persistence, and interaction logic remain independent from that temporary visualization.

## Prompt Output Contract (Diagnostics/Performance Ideas)
- Copy-ready `@audit` prompt template:
  - Symptom signature:
    - Chunk streaming stutters, repeated chunk regeneration, slow interaction queries, or duplicated world-object materialization.
  - Reproduction steps:
    - Start near chunk boundary, move across multiple chunk edges, trigger at least one interaction with a generated world object, then save/reload and repeat.
  - Environment/build:
    - Rust workspace, Bevy app runtime, chunked planar world generation, deterministic seed, persistence enabled.
  - Captured evidence summary:
    - Chunk load/unload counts, generation count per chunk key, persistence hit/miss counts, number of active rendered chunks, entity-index query counts, world-object materialization counts.
  - Regressions to avoid:
    - Full-world scans, regeneration of already-persisted chunks, duplicated source-of-truth between chunk snapshots and ECS entities, filesystem logic leaking into `mm-core`.
  - Capture validity (valid/invalid + reason):
    - Valid only if logs distinguish cache hit vs generation miss and chunk keys are included in the counters.

## Regressions to Avoid
- Do not let `mm-app` become the owner of world generation rules or chunk truth.
- Do not put persistence/file I/O into `mm-core`.
- Do not use one “god” spatial index for both persistent world data and dynamic entities.
- Do not make every procedural object a permanent ECS entity.
- Do not regenerate chunks every time they become visible if persistent data already exists.
- Do not hardcode theme colors, chunk sizes, or generation thresholds across multiple modules.
- Do not import spherical-world assumptions from `spatial/` unchanged.
- Do not reintroduce dense tile-map thinking into the first chunk model when a coarse base theme plus stamps is sufficient.
- Do not use `thread_rng`, unordered map iteration, or any generation step whose output depends on processing order.

## Validation Plan
- [x] `cargo check` passes for the workspace after introducing the world module scaffolding.
- [x] Deterministic tests prove same seed + chunk key => same chunk output.
- [x] Deterministic tests prove generating chunk A then B yields the same outputs as generating chunk B then A.
- [x] Deterministic tests prove no unordered collection iteration can perturb serialized chunk output.
- [x] Mutation tests prove persistent deltas override generated baseline without changing unrelated data.
- [x] Spatial tests prove entity index updates do not duplicate entities across chunk moves.
- [x] Runtime smoke test proves only nearby chunks are materialized/rendered.
- [x] Save/reload test proves interacted chunks are loaded from persisted state instead of regenerated from scratch.

## Definition of Done Checklist
- [x] Cargo checks pass for affected crates
- [x] No half-built runtime flows or dead paths
- [x] Signals are non-zero/sane in active scenario
- [x] Implementation maps to stated user goal

## Execution Log (Living History)
- 2026-03-27 16:44 UTC+01:00 Read `doc/architecture.md`, `doc/CHANGELOG.md`, and `doc/execs/exec_idea.md` before proposing the new world direction.
- 2026-03-27 16:45 UTC+01:00 Inspected `crates/mm-app/src/lib.rs` and confirmed the current terrain path was a hardcoded app-side prototype rather than a reusable world system.
- 2026-03-27 16:46 UTC+01:00 Reviewed the reference `spatial/` module and identified the transferable pieces: `ChunkKey`, dynamic entity indexing patterns, and chunk-tracking concepts; rejected direct reuse of spherical traversal logic.
- 2026-03-27 16:47 UTC+01:00 Removed the temporary terrain/tile-map rendering path from `mm-app` and revalidated the workspace with `cargo check`.
- 2026-03-27 16:48 UTC+01:00 Chose shared chunk vocabulary plus separate world/entity indices as the preferred architecture to avoid a single over-coupled spatial structure.
- 2026-03-27 17:04 UTC+01:00 Resolved first-slice planning decisions: hard chunk boundaries, flat-file app-layer persistence (`RON` for debug-oriented development, `bitcode` for release-oriented builds), a 3x3 active chunk window target, and colored debug visualization instead of asset-heavy terrain rendering.
- 2026-03-27 17:16 UTC+01:00 Locked the final planning choices: chunk output starts as a coarse base theme plus deterministic `ProcAsset` stamps, and generation must be chunk-order-independent with explicit per-chunk seeded RNG and ordered data handling from the start.
- 2026-03-27 17:38 UTC+01:00 Executed the first slice: added `mm-core::world` with planar chunk vocabulary, deterministic coarse-base generation, stable procedural asset ids, mutation overlays, and a pure in-memory world store; then wired `mm-app` to stream nearby chunks, persist chunk state to flat files, maintain a separate live entity chunk index, render debug gizmos, and prove interaction via blocking rocks, destructible bushes, and swim-surface feedback.

## Final Outcome
The first procedural-world execution slice is now in place. `mm-core` owns a dedicated `world` domain with planar chunk keys, coarse base-theme chunk output, deterministic per-chunk generation, stable procedural object identity, and mutation overlays. `mm-app` remains the thin runtime boundary, now responsible for chunk streaming, flat-file persistence, debug-gizmo rendering, and the live entity chunk index. The next recommended step is to run the new world-specific tests and then deepen the domain with richer chunk content, stronger persistence round-trip coverage, and more explicit runtime/entity realization rules.

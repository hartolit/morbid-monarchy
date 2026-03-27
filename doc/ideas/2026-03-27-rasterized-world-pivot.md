# Idea: Rasterized Chunk World Pivot

## Raw Idea Input
I have successfully implemented the first slice of the chunked world domain, but I need to fix a rendering bug and make a major architectural pivot towards a dynamic, pixel-rasterized world.

Task 1: Fix the Base Layer Rendering Bug
Currently, the chunk's base layer is only visible at the edges and flickers/changes colors when the player walks.

- Fix the Z-index so the chunk base layer is distinctly behind entities and assets (e.g., z: -1.0 or lower).
- Check the chunk streaming logic. Make sure we aren't constantly despawning/regenerating the same chunk due to integer rounding errors in the player's grid coordinate calculation.

Task 2: Camera Follow System
Implement a simple system in mm-app where the primary 2D camera matches the X/Y translation of the Player entity.

Task 3: Pivot to Pixel-Rasterized Chunks (The Big Feature)
I want players and enemies to leave persistent marks on the world (e.g., blood splatters) and interact with the world pixel-by-pixel.
We need to update mm-core to support a rasterized material grid.

- Define Material Grid: Instead of a chunk just having a broad theme, give ChunkData a fixed-size 2D array (e.g., 64x64 or whatever size fits our world units) representing pixels/voxels. Create an enum like WorldPixel { Empty, Grass, Dirt, Rock, Blood }.
- Procedural Pixel Generation: Update the deterministic generation logic. Based on the chunk's overarching theme, run a basic loop (perhaps using some simple noise or pattern logic) to populate this pixel array deterministically.
- The Translation Layer (Baking): Create a function in mm-core that allows a dynamic event to modify a chunk's grid. For example: chunk.set_pixel(local_x, local_y, WorldPixel::Blood).
- Enemy Spawning & Splatter: Spawn a basic enemy entity. When damaged/killed, it should trigger an event that calculates its world position, maps that to a specific chunk and local pixel coordinate, and overwrites that pixel with WorldPixel::Blood.
- Delta Persistence: Ensure our persistence logic does not save the entire pixel array if it doesn't have to. The mm-core state should ideally track a "delta" of modified pixels so we only serialize the blood splatters and destroyed terrain, applying them over the deterministic base layer when reloading.

Please start by drafting the updated ChunkData and WorldPixel structs in mm-core so we can agree on the memory layout before implementing the Bevy rendering updates. By this I mean a detailed @idea report.

## User Goal (Plain Language)
Evolve the current coarse chunk world into a deterministic pixel-raster world without polluting `mm-core` with app/runtime concerns. Success means the project gets a chunk data model that can express a dense material field, can be mutated at pixel precision, can persist only the changed pixels instead of entire baked chunks, and can later support a stable Bevy-side renderer, camera follow, and enemy blood splatter flow without repeated chunk churn or rendering-order artifacts.

## Context
The current shipped slice has `mm-core` generating a coarse `ChunkSnapshot` made of `theme`, `base_layer`, and procedural asset stamps, while `mm-app` owns chunk streaming, persistence I/O, and debug rendering. The requested pivot keeps that boundary, but upgrades the world domain from coarse chunk semantics to a deterministic raster material field. This change also intersects with later app-side work: base layer draw order, chunk-stream coordinate stability, camera follow, and event-driven splatter rendering.

## Evidence Inventory
- Available evidence:
  - `doc/architecture.md` requires `mm-core` to stay deterministic, pure, and free of filesystem/bootstrap/rendering concerns.
  - `crates/mm-core/src/world/types.rs` currently models chunk truth as `ChunkSnapshot { key, theme, base_layer, assets }` plus `ChunkMutation { removed_object_ids }` and `ChunkState { snapshot, mutation }`.
  - `crates/mm-core/src/world/generation.rs` already provides deterministic per-chunk generation via `WorldConfig`, `ChunkKey`, and a per-chunk seeded `ChaCha8Rng`.
  - `crates/mm-app/src/world_runtime.rs` currently streams chunks around the player by calling `ChunkKey::from_world_position(transform.translation, config.chunk_world_size)` and removing all non-active chunks from the in-memory `WorldStore`.
  - `ChunkKey::from_world_position` currently uses floating-point division plus `floor()`, which is acceptable for coarse chunks but is a likely instability seam once raster-space interactions depend on exact world-to-grid mapping at chunk boundaries.
  - `crates/mm-app/src/world_persistence.rs` currently serializes the entire `ChunkState`, which is acceptable for a coarse snapshot but directly conflicts with the desired delta-only persistence strategy once chunks contain dense raster material arrays.
  - `crates/mm-app/src/lib.rs` already has a `Camera2d` spawn site and a clean app-owned update schedule, so camera follow belongs there rather than in `mm-core`.
  - The uploaded screenshot shows a visible chunk-base rendering issue not fully represented by the tracked repository snapshot; the current checked-in app path still renders world debug geometry rather than a dense raster base layer, so some bug-specific evidence is still outside the code currently indexed.
- Missing evidence required for decision:
  - Final chunk raster resolution target and the intended ratio between chunk world units and raster pixels.
  - Whether raster rendering in `mm-app` will use one texture per chunk, one mesh/sprite atlas approach, or a debug-image upload path first.
  - Expected long-term material vocabulary beyond `Empty`, `Grass`, `Dirt`, `Rock`, and `Blood`.
  - Whether dynamic terrain destruction will only change material kind or will later require per-pixel durability, temperature, wetness, ownership, or decals.
  - Whether the enemy-splatter event should mark a single pixel, a radius/brush, or a shape mask.
  - Whether chunk persistence should remain one file per chunk or later move to save slots/regions.

## Constraints (from doc/architecture.md)
- `mm-core` must remain the source of truth for world-domain logic, coordinate mapping, deterministic chunk generation, and pixel mutation rules.
- `mm-app` must remain the thin boundary for Bevy rendering, camera orchestration, runtime chunk streaming, persistence I/O, and enemy ECS behavior.
- No filesystem coupling, renderer state, texture handles, or Bevy asset logic belongs in `mm-core`.
- Stable defaults should be centralized in typed config/constants rather than scattered as ad hoc numbers.
- The solution should respect semantic cohesion: world-domain modeling and generation belong together, while app runtime/render concerns stay in `mm-app`.
- The pivot must not silently reintroduce a monolithic “god object” that mixes deterministic base data, runtime cache state, persistence transport, and render-upload metadata into one struct.

## Assumptions
- The world remains planar for gameplay, with `Vec3.z` used for render layering rather than physical chunk depth in the first raster slice.
- A chunk-local raster should be fixed-size and dense in memory so deterministic generation and local sampling stay cheap.
- A `64 x 64` chunk raster is the best initial target because it is large enough to produce visible per-pixel structure while remaining small enough for trivial in-memory dense storage (`4096` pixels per chunk).
- The current default `chunk_world_size` of `256.0` world units can map cleanly to a `64 x 64` raster, producing `4.0` world units per pixel.
- Dynamic changes should be modeled as sparse overrides on top of a deterministic baked raster, not as full chunk rewrites.
- A later renderer can upload chunk textures from core-provided material grids, but the core should not know or care how that upload happens.
- Player and enemy splatter should target chunk-local integer pixel coordinates derived from a single authoritative world-to-raster transform owned by `mm-core`.

## Unknowns / Questions
- [ ] [high] Should the fixed raster size be globally invariant (`64 x 64` for every chunk) or configurable per world/save via `WorldConfig` while still fixed within a given save?
- [ ] [high] Should `BaseLayer` survive as a coarse semantic hint, or should it be fully replaced by raster-derived material semantics to avoid duplicate truth?
- [ ] [medium] Should procedural assets remain separate high-level stamps after rasterization, or should some assets become baked directly into the material grid while only interactable entities remain separate?
- [ ] [medium] Should blood be a permanent material kind or a separate decal/overlay layer once more visual effects are introduced?
- [ ] [medium] Should the first enemy splatter event write a single pixel or a deterministic small brush mask?
- [ ] [low] Does `ChunkKey.z` still need to participate in chunk streaming now that the active gameplay world is strictly 2D?

## Goal -> Signal Traceability
| Goal | Required signal | Collection point | Validation threshold |
| --- | --- | --- | --- |
| Agree on chunk memory layout before implementation | One active idea file defines concrete `WorldPixel`, raster-grid, delta, and ownership choices | `doc/ideas/2026-03-27-rasterized-world-pivot.md` | Exact struct-level direction chosen and justified |
| Preserve deterministic base generation | Same world seed + chunk key produces identical raster base grid | future `mm-core::world` tests | Pixel-for-pixel stable output for repeated generation |
| Persist only dynamic changes | Save payload excludes dense generated base raster | future `world_persistence` serialization tests | Persisted chunk payload contains only chunk identity + deltas |
| Prevent chunk streaming churn | World-to-chunk mapping is integer-stable across movement and chunk boundaries | future core coordinate tests + app runtime tests | Crossing a boundary changes chunk key once, not repeatedly |
| Enable future splatter flow | World position maps to `(ChunkKey, local_pixel)` through one authoritative helper | future core coordinate/mutation tests | Event-driven splatter writes expected pixel and survives reload |
| Keep rendering/app concerns out of core | No texture/camera/render types appear in `mm-core` | code review + compiler boundary | `mm-core` remains pure Rust domain logic |

## Options Explored
### Option A - Dense Full Raster Stored and Persisted Inside Every Chunk State
- Architecture fit:
  - Weak to moderate. The dense raster itself fits core ownership, but persisting the full generated array inside every serialized chunk duplicates deterministic data and couples runtime state to storage waste.
- Performance risk:
  - Medium. Runtime reads are cheap, but persistence size grows linearly with all active/generated chunks even when only a few pixels changed.
- Regression vectors:
  - Save files bloat rapidly.
  - Every mutation risks rewriting whole-chunk payloads.
  - It becomes harder to distinguish base generation bugs from persistence bugs.
- Complexity/cost:
  - Lowest implementation cost.
- Verdict: Feasible with Mitigations

### Option B - Deterministic Dense Base Raster Plus Sparse Sorted Delta Overlay
- Architecture fit:
  - Strong. `mm-core` owns the generated dense material truth and the mutation rules, while `mm-app` persists only the overlay/delta transport.
- Performance risk:
  - Low to moderate. Reads require either overlay lookup or a lightweight rebake/apply step, but the core invariant stays clean and persistence stays compact.
- Regression vectors:
  - Requires careful API design so callers do not accidentally mutate the generated base grid without recording deltas.
  - Requires stable ordering/indexing for delta serialization.
- Complexity/cost:
  - Moderate. Slightly more code, much better long-term shape.
- Verdict: Feasible

### Option C - Bit-Packed Material IDs With Separate Overlay Planes From Day One
- Architecture fit:
  - Moderate. Very memory efficient, but over-optimizes before the material vocabulary and mutation workload are proven.
- Performance risk:
  - Medium to high. The extra packing/unpacking and multi-plane logic can complicate deterministic generation, debugging, and serialization before the need is demonstrated.
- Regression vectors:
  - Harder debugging in `RON`.
  - Higher chance of indexing bugs and mismatched planes.
  - Slower iteration when material semantics inevitably change during gameplay prototyping.
- Complexity/cost:
  - Highest upfront complexity.
- Verdict: High Regression Risk

## Preferred Direction
Select **Option B**: keep a **dense deterministic base raster in memory** and pair it with a **sparse, sorted delta overlay**.

### Recommended memory layout
- `WorldPixel`
  - Represent it as a compact `#[repr(u8)]` enum.
  - Initial vocabulary:
    - `Empty = 0`
    - `Grass = 1`
    - `Dirt = 2`
    - `Rock = 3`
    - `Blood = 4`
  - Reasoning:
    - One byte per pixel is explicit, debuggable, serialization-friendly, and sufficient for the current material set.
    - It avoids premature bit-packing while keeping room for deterministic generation and direct texture-palette lookup later.
  - Domain behavior should hang off the enum, not off app code:
    - `is_solid()`
    - `traversal()`
    - `blocks_movement()`
    - `palette_index()` or equivalent renderer-facing stable identifier if needed later.

- Chunk raster constants / config
  - Introduce a core-owned invariant for the first slice:
    - `CHUNK_PIXEL_SIZE: u16 = 64`
    - `CHUNK_PIXEL_COUNT: usize = 4096`
  - Keep the chunk world span in `WorldConfig`.
  - Add a helper that derives `world_units_per_pixel()` from `chunk_world_size / CHUNK_PIXEL_SIZE as f32`.
  - Keep raster dimensions global per world/save, not per chunk instance, so `ChunkData` does not duplicate shape metadata on every chunk.

- `ChunkData`
  - Replace the current coarse `ChunkSnapshot` payload with a deterministic chunk data type that owns the baked base raster:
  - `key: ChunkKey`
  - `theme: ChunkTheme`
  - `materials: Box<[WorldPixel; CHUNK_PIXEL_COUNT]>`
  - `assets: Vec<ProcAsset>`
  - Why `Box<[WorldPixel; CHUNK_PIXEL_COUNT]>`:
    - Keeps a fixed-size dense array.
    - Avoids inflating stack copies when chunks are cloned/moved.
    - Maintains cache-friendly contiguous storage.
    - At `64 x 64` and `repr(u8)`, the dense material grid costs about `4 KiB` per chunk before allocator/container overhead, which is entirely acceptable for the current active-window scale.

- `PixelDelta`
  - Store per-pixel edits sparsely instead of rewriting the base raster to disk.
  - Recommended shape:
    - `local_index: u16`
    - `pixel: WorldPixel`
  - Use a flattened index instead of `(x, y)` pairs to reduce stored size and simplify sorting/comparison.
  - For a `64 x 64` raster, `u16` easily covers the entire chunk (`0..4095`).

- `ChunkDelta`
  - Evolve `ChunkMutation` into a more general sparse overlay model:
    - `removed_object_ids: Vec<WorldObjectId>`
    - `pixel_overrides: Vec<PixelDelta>`
  - Keep `pixel_overrides` sorted by `local_index` and de-duplicated.
  - Mutation API rule:
    - If a new pixel matches the deterministic base pixel again, remove the override instead of storing a no-op override.

- `ChunkState`
  - Runtime composite only:
    - `data: ChunkData`
    - `delta: ChunkDelta`
  - Read APIs should resolve a pixel as:
    - override if present
    - otherwise deterministic base raster pixel
  - Write APIs should go through core-owned helpers such as:
    - `set_pixel(local_x, local_y, pixel)`
    - `set_pixel_by_index(local_index, pixel)`
    - `pixel_at(local_x, local_y)`
    - `pixel_at_world(world_position, &WorldConfig)` via coordinate helpers

### Recommended coordinate model pivot
The raster pivot is the right moment to stop treating chunk lookup as “just float world position divided by chunk size.”

Introduce core helpers that make integer grid space the authoritative bridge:
- Convert world `Vec2/Vec3` to world-pixel coordinates using `world_units_per_pixel()`.
- Convert world-pixel coordinates to:
  - `ChunkKey` via `div_euclid(CHUNK_PIXEL_SIZE as i32)`
  - chunk-local pixel coordinates via `rem_euclid(CHUNK_PIXEL_SIZE as i32)`
- Keep all chunk-local raster addressing integer-based once the first conversion is made.

This matters for two later tasks:
- It makes chunk streaming less likely to thrash near boundaries because the same authoritative mapping can be reused everywhere.
- It gives splatter/damage events an exact, deterministic target pixel instead of relying on repeated floating-point floor logic in multiple systems.

### Recommended stance on existing coarse fields
- `ChunkTheme` should remain. It is still useful as the top-level generation seed/theme selector.
- `BaseLayer` should **not** remain as independent chunk truth in the rasterized design. It becomes redundant once the raster material field is authoritative.
- `ProcAsset` can survive initially for large interactables or stamped world objects, but generation should gain the ability to bake some material directly into the raster rather than representing everything as coarse stamps.

### Recommended persistence boundary
To satisfy the delta-persistence goal cleanly, do **not** keep serializing full `ChunkState` once `ChunkData` contains a dense raster.

Instead:
- `mm-core` should define the pure delta/state vocabulary.
- `mm-app` persistence should serialize only a compact payload such as:
  - `key: ChunkKey`
  - `delta: ChunkDelta`
- On load:
  - regenerate `ChunkData` deterministically from `(WorldConfig, ChunkKey)`
  - apply `ChunkDelta`
  - reconstruct runtime `ChunkState`

This keeps deterministic base generation and persistence transport cleanly separated.

## Decision Checkpoint (if 2+ options remain plausible)
- Discriminating experiment:
  - Implement the core-only `WorldPixel`/`ChunkData`/`ChunkDelta` model and run a serialization comparison between:
    - full dense-chunk persistence, and
    - delta-only persistence after applying a tiny blood splatter.
- Falsification signal for Option A:
  - If a one-pixel blood write still forces an entire dense raster payload to serialize and materially increases per-chunk save size, Option A should be rejected.
- Falsification signal for Option B:
  - If overlay lookup or apply-on-load logic becomes complex enough to duplicate pixel truth or create divergent read/write paths, the API surface needs tightening before app integration.
- Falsification signal for Option C:
  - If debugging, tests, or persistence inspection become harder than the memory saved is worth, bit-packing remains premature.

## Delivery Mode
- Selected mode: Phased
- Rationale:
  - The pivot spans core world modeling, deterministic generation, persistence semantics, coordinate math, rendering, and gameplay events. A phased rollout is safer than mixing the memory-model decision with app-side rendering and bug fixes in one jump.

## Execution Plan (Approval Gate)
- [x] Replace coarse chunk snapshot modeling in `mm-core` with a raster-first `ChunkData` plus sparse `ChunkDelta` design.
- [x] Add core-owned integer-safe world-to-chunk and world-to-local-pixel mapping helpers derived from `WorldConfig`.
- [x] Update deterministic chunk generation so each chunk bakes a stable `64 x 64` base material raster from theme + seed.
- [x] Add core mutation helpers that write sparse pixel overrides and erase no-op overrides.
- [x] Update `mm-app` persistence to serialize only chunk delta payloads and reconstruct runtime chunk state by regeneration plus overlay application.
- [x] Fix the base-layer rendering bug in `mm-app` by making the raster chunk visual render behind entities/assets and by eliminating any unstable chunk-key churn in streaming/render ownership.
- [x] Add a camera-follow system in `mm-app` that copies the player's X/Y translation onto the primary `Camera2d`.
- [x] Add a basic enemy flow in `mm-app` that emits a splatter event on damage/death, maps world position to chunk-local pixel coordinates through core helpers, and paints `WorldPixel::Blood`.
- [x] Validate determinism, boundary stability, persistence size, reload correctness, and splatter persistence with focused tests.

## Module Ownership Map
- `crates/mm-core/src/world/types.rs`: owns `WorldPixel`, raster constants/helpers, chunk-local pixel coordinate types, `ChunkData`, `PixelDelta`, `ChunkDelta`, and `ChunkState` mutation/read APIs.
- `crates/mm-core/src/world/generation.rs`: owns deterministic base raster baking from theme/seed and any helper logic for material fill patterns.
- `crates/mm-core/src/world/mod.rs`: exports the new raster-first world vocabulary.
- `crates/mm-core/src/lib.rs`: re-exports the updated world API surface to app consumers.
- `crates/mm-app/src/world_runtime.rs`: owns app runtime streaming, chunk visual lifecycle, camera follow system, enemy-to-world mutation event handling, and chunk-boundary stability integration.
- `crates/mm-app/src/world_persistence.rs`: owns delta-only chunk persistence payloads and regeneration-on-load orchestration.
- `crates/mm-app/src/lib.rs`: wires Bevy startup, camera spawn tagging if needed, and scheduling for follow/render/runtime systems.
- `doc/ideas/2026-03-27-rasterized-world-pivot.md`: single source of truth for this pivot until implementation is approved/executed.

## Feature Flag Plan (Debug/Diagnostics)
- Compile-time flags:
  - No new compile-time feature is required for the core memory layout.
  - Continue using debug-vs-release persistence format switching only at the app boundary if desired.
- Runtime flags:
  - Optional future debug toggle in `mm-app` for chunk bounds, pixel-grid overlays, and chunk-key labels.
- Release-default behavior:
  - The renderer should show rasterized chunk textures/sprites without debug overlays by default, while `mm-core` remains completely unaware of that choice.

## Prompt Output Contract (Diagnostics/Performance Ideas)
- Copy-ready `@audit` prompt template:
  - Symptom signature:
    - Chunk base layer flickers or changes color while walking; chunks appear to reload at boundaries; blood splatters disappear after crossing chunk edges or reloading.
  - Reproduction steps:
    - Start at chunk origin, walk slowly across positive and negative X/Y boundaries, observe base layer stability, damage an enemy near a chunk edge, leave and re-enter the chunk, then reload the app.
  - Environment/build:
    - Rust Cargo workspace, `mm-core` deterministic raster chunks, `mm-app` Bevy runtime, flat-file chunk persistence, debug build.
  - Captured evidence summary:
    - Per-frame player world position, derived world-pixel coordinate, derived `ChunkKey`, chunk load/save counts, chunk despawn/regenerate counts, per-chunk override counts, base-layer render entity count, and camera follow target/actual positions.
  - Regressions to avoid:
    - Reintroducing float-only chunk mapping in multiple places, persisting full baked chunk rasters, moving render concerns into `mm-core`, and producing divergent coordinate math between player streaming and splatter events.
  - Capture validity (valid/invalid + reason):
    - Valid only if logs show the same world position mapping pipeline used by streaming, rendering, and splatter writes; invalid if each system computes chunk-local coordinates independently.

## Regressions to Avoid
- Do not keep `BaseLayer` as a second authoritative terrain truth once the raster material grid exists.
- Do not serialize full generated material arrays when only sparse overrides changed.
- Do not let `mm-app` invent its own world-to-pixel mapping separate from `mm-core`.
- Do not store renderer upload metadata or texture handles in core chunk types.
- Do not mutate the baked base raster in place and then lose the ability to distinguish generation truth from dynamic delta.
- Do not introduce per-pixel metadata structs unless gameplay proves the need; keep first-slice pixels as material-only.
- Do not rely on floating-point chunk rounding in multiple systems once pixel-precise interactions are introduced.
- Do not overfit for bit-packing or SIMD before the simple `repr(u8)` model is proven insufficient.

## Validation Plan
- [x] Add core tests proving deterministic `ChunkData` raster generation for identical seed/key pairs.
- [x] Add core tests proving world-position -> `(ChunkKey, local_pixel)` mapping is stable across positive and negative chunk boundaries.
- [x] Add core tests proving `set_pixel` records, updates, de-duplicates, and clears `PixelDelta` entries correctly.
- [x] Add app persistence tests proving a chunk with a tiny blood splatter serializes only delta payload and reconstructs the same effective chunk after reload.
- [x] Add app/runtime tests proving boundary crossing does not repeatedly load/despawn the same chunk when the player hovers near edges.
- [x] Add app/runtime tests proving camera X/Y follows the player while preserving camera Z.
- [x] Add app/runtime tests proving enemy splatter writes the expected local pixel and remains visible after chunk reload.

## Definition of Done Checklist
- [x] Cargo checks pass for affected crates
- [x] No half-built runtime flows or dead paths
- [x] Signals are non-zero/sane in active scenario
- [x] Implementation maps to stated user goal

## Execution Log (Living History)
- 2026-03-27 21:50 UTC+01:00 Read `doc/architecture.md`, `doc/CHANGELOG.md`, and `doc/execs/exec_idea.md` to satisfy the workspace documentation and `@idea` workflow requirements before proposing changes.
- 2026-03-27 21:52 UTC+01:00 Inspected `crates/mm-core/src/world/types.rs`, `crates/mm-core/src/world/generation.rs`, `crates/mm-core/src/world/mod.rs`, `crates/mm-core/src/lib.rs`, `crates/mm-app/src/world_runtime.rs`, `crates/mm-app/src/world_persistence.rs`, and `crates/mm-app/src/lib.rs` to map current chunk ownership, deterministic generation, persistence, and runtime streaming responsibilities.
- 2026-03-27 21:55 UTC+01:00 Identified the main architectural tension: the current coarse chunk snapshot shape and full-state persistence are fine for theme-plus-asset chunks but are the wrong persistence boundary for a dense raster chunk model.
- 2026-03-27 21:58 UTC+01:00 Compared three raster memory-layout directions and selected dense deterministic base raster plus sparse sorted delta overlay as the best fit for purity, persistence efficiency, and debuggability.
- 2026-03-27 22:00 UTC+01:00 Locked the preferred data-shape direction: `WorldPixel` as `repr(u8)`, `64 x 64` dense chunk material storage, flattened sparse `PixelDelta` overrides, core-owned integer-safe coordinate mapping, and app-owned delta-only persistence.
- 2026-03-27 22:01 UTC+01:00 Deferred implementation pending explicit approval (`Execute`) per the `@idea` workflow.
- 2026-03-27 22:05 UTC+01:00 Executed the `mm-core` raster pivot: replaced coarse chunk snapshot/base-layer truth with dense `WorldPixel` materials, added `ChunkLocalPixel`, `ChunkPixelPosition`, sparse `ChunkDelta`/`PixelDelta`, and routed `ChunkKey::from_world_position` through a shared world-to-pixel mapping helper.
- 2026-03-27 22:08 UTC+01:00 Upgraded deterministic chunk generation to bake `64 x 64` material rasters from theme plus per-chunk seeded world-pixel patterns, and added tests proving deterministic output plus delta-backed pixel mutation clearing.
- 2026-03-27 22:12 UTC+01:00 Moved app persistence to delta-only chunk payloads, regenerating `ChunkData` on load and persisting only sparse runtime overrides or removed-object ids.
- 2026-03-27 22:16 UTC+01:00 Rebuilt `mm-app` runtime around the raster model: stable chunk streaming now reuses shared core coordinate mapping, chunk base layers render as image-backed sprites at `z = -10.0`, player surface sampling resolves from raster pixels, and collision now respects blocking raster materials in addition to blocking assets.
- 2026-03-27 22:19 UTC+01:00 Added app-side camera follow, spawned a basic enemy, and implemented an event-like pending-splatter flow that maps enemy death positions through core pixel coordinates and persists `WorldPixel::Blood` into chunk deltas.
- 2026-03-27 22:21 UTC+01:00 Validated the shipped slice with passing `cargo test` and `cargo check`, including new tests for camera follow, splatter persistence, chunk persistence cleanup, and enemy splatter queuing.

## Final Outcome
Executed.

`mm-core` now owns a raster-first chunk model with deterministic `64 x 64` material grids, sparse pixel/object deltas, and integer-safe world-to-chunk/pixel mapping helpers. `mm-app` now renders chunk rasters as image-backed base sprites behind actors, follows the player with the primary 2D camera, persists only chunk deltas, and supports a basic enemy splatter flow that paints and reloads persistent blood pixels. Workspace validation passed with `cargo test` and `cargo check`.

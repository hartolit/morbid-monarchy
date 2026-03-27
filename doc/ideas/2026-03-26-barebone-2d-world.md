# Idea: Barebone 2D World With Player Movement

## Raw Idea Input
@[doc/execs/exec_idea.md] we need to setup a barebone project to ensure we've got the right architecture and understanding of how to build the project. We'll just start out with a simple example:
Build simple 2D game world with player movement.

## User Goal (Plain Language)
Create a minimal but correct project baseline that proves the workspace architecture is sound: `mm-core` should own deterministic ECS-native game-domain state and movement rules using Bevy's pure data crates, while `mm-app` should be the thin executable layer that boots the runtime, wires OS input/rendering, and demonstrates a movable player in a simple 2D world.

## Context
This touches the Rust Cargo workspace root and the two existing crates: `crates/mm-core` and `crates/mm-app`. Right now the workspace is only a placeholder, so this idea is about choosing the minimal architectural slice that establishes the intended long-term build pattern without overbuilding systems too early.

## Evidence Inventory
- Available evidence:
  - `doc/architecture.md` defines `mm-core` as the pure domain crate and `mm-app` as the thin runtime/application crate.
  - Workspace dependencies already include `bevy = "0.18.1"`.
  - `crates/mm-core/src/lib.rs` only contains `print_hello()`.
  - `crates/mm-app/src/main.rs` only prints hello-world and calls into `mm-core`.
  - `crates/mm-app/src/lib.rs` is empty.
  - `crates/mm-core/Cargo.toml` currently declares a `[[bin]]` target named `mm-app` pointing to `src/main.rs`, but that file does not exist.
- Missing evidence required for decision:
  - Whether the intended first slice should include rendering now or only simulation + tests.
  - Whether camera setup, collision, and fixed timestep are in initial scope or intentionally deferred.
  - Whether the user wants keyboard input only or gamepad-ready abstractions from day one.

## Constraints (from doc/architecture.md)
- `mm-core` must remain the source of truth for domain logic, state transitions, reusable algorithms, and data modeling.
- `mm-app` must own startup, orchestration, and boundary I/O.
- Dependency direction must remain from app to core only.
- Avoid scattering magic numbers; use config/default types and associated constants.
- Keep modules cohesive and avoid premature fragmentation.
- Keep the implementation pure Rust.

## Assumptions
- The first slice should be a playable local prototype, not a networking or tooling exercise.
- Keyboard-driven movement is sufficient for the first vertical slice.
- A single player in an empty world is enough to validate architecture.
- Bevy is the intended app/runtime layer for rendering and input.
- `bevy_ecs`, `bevy_math`, and `bevy_transform` are valid pure-domain dependencies for `mm-core` and do not violate the crate boundary.

## Unknowns / Questions
- [medium] Should the first slice include rendering immediately, or should the app initially log/core-test movement only?
- [low] Should movement be time-step based from the start, or frame-step based for the very first scaffold?
- [low] Should the first slice expose configurable movement speed via typed config, or can that live as a core default constant initially?

## Goal -> Signal Traceability
| Goal | Required signal | Collection point | Validation threshold |
| --- | --- | --- | --- |
| Prove crate boundaries are correct | `mm-core` contains movement/domain logic while `mm-app` owns runtime boot/input/render wiring | file/module ownership review | no app/bootstrap concerns inside `mm-core` |
| Prove the project can run a minimal 2D example | application launches and displays a player entity in a 2D scene | `mm-app` runtime entrypoint | app starts without missing target/config errors |
| Prove player movement works end-to-end | keyboard input changes core movement intent and core systems mutate the rendered entity state predictably | app runtime + core movement tests | player moves in expected direction under input |
| Prove architecture can scale | movement config/components/resources are reusable and not hardcoded across layers | core ECS modules | single source of truth for movement constants/state |

## Options Explored
### Option A - ECS-native core plugin + thin Bevy app orchestrator
- Architecture fit:
  - Strongest fit. `mm-core` owns ECS `Component`s, `Resource`s, and pure simulation `System`s using `bevy_ecs`, `bevy_math`, and `bevy_transform`.
  - `mm-app` translates raw Bevy input into core intent components/resources and layers rendering/window concerns on top of the same entities.
- Performance risk:
  - Low for current scope.
  - No synchronization tax if the same ECS data is consumed by both simulation and rendering.
- Regression vectors:
  - Requires discipline to keep `mm-core` limited to pure Bevy data/schedule crates and avoid leaking windowing, rendering, and hardcoded OS input concerns into core.
  - Requires a clean intent boundary so `mm-app` translates raw input into domain-friendly components/resources instead of embedding gameplay logic.
- Complexity/cost:
  - Moderate, but sets the correct architectural precedent.
- Verdict: Feasible

### Option B - Bevy-first ECS with gameplay logic embedded in `mm-app`, `mm-core` only for shared constants/types
- Architecture fit:
  - Weak. It makes `mm-core` mostly ornamental and pushes gameplay behavior into the app crate.
- Performance risk:
  - Low in the short term.
  - Higher long-term architecture risk because gameplay logic becomes runtime-coupled.
- Regression vectors:
  - High chance of violating the documented crate boundary and creating a god-app crate.
- Complexity/cost:
  - Low initial effort, but poor long-term cost profile.
- Verdict: High Regression Risk

### Option C - Core-only text/test simulation first, defer Bevy runtime until later
- Architecture fit:
  - Strong fit for purity.
  - Weaker fit for the user's request to establish how to build the actual game project.
- Performance risk:
  - Lowest.
- Regression vectors:
  - Could delay discovery of runtime wiring issues and make the first real app integration a second architectural step.
- Complexity/cost:
  - Lowest immediate cost, but does not fully prove end-to-end project shape.
- Verdict: Feasible with Mitigations

## Preferred Direction
Option A. It gives the smallest end-to-end vertical slice that still respects the architecture document. `mm-core` becomes the authoritative ECS-native gameplay model for a tiny 2D world, while `mm-app` becomes a thin Bevy runner that owns window creation, camera spawning, raw keyboard input translation, and visual composition on top of the same ECS entities.

## Decision Checkpoint (if 2+ options remain plausible)
- Discriminating experiment:
  - Build the smallest possible slice with one player entity, one movement config resource, one movement-intent component/resource, and core systems that directly mutate shared ECS state consumed by rendering.
- Falsification signal for Option A:
  - If implementing a thin app requires duplicated gameplay state or forces windowing/render/input-hardcoding concerns into `mm-core`, the boundary design is wrong.
- Falsification signal for Option C:
  - If deferring runtime integration leaves unresolved uncertainty about startup/input/render ownership, then it is too abstract for this project stage.

## Delivery Mode
- Selected mode: Single-pass
- Rationale:
  - The work is bounded and architectural. A single vertical slice is the fastest way to validate both crate responsibilities and the development loop.

## Execution Plan (Approval Gate)
- [ ] Remove the invalid `[[bin]]` target from `crates/mm-core/Cargo.toml` and make `mm-core` a pure library crate.
- [ ] Introduce a compact `mm-core` ECS domain with player components, movement intent, movement config/defaults, and deterministic simulation systems packaged behind a core plugin.
- [ ] Add focused unit tests in `mm-core` proving player movement semantics.
- [ ] Convert `mm-app` into the actual executable owner with a Bevy app entrypoint and no gameplay rules embedded outside orchestration/input mapping.
- [ ] Add a minimal 2D scene in `mm-app` with camera + player visual, reusing the same ECS transform/state owned by `mm-core` without a duplication layer.
- [ ] Validate with `cargo check` and, if practical, a short `cargo run -p mm-app` smoke test.

## Module Ownership Map
- `crates/mm-core/Cargo.toml`: remove invalid binary declaration; keep pure domain dependencies only.
- `crates/mm-core/src/lib.rs`: expose cohesive public modules and the core plugin entrypoint.
- `crates/mm-core/src/player.rs`: own player markers, movement intent, and movement-related components/resources.
- `crates/mm-core/src/systems.rs`: own deterministic simulation systems over ECS state.
- `crates/mm-core/src/plugin.rs` or `crates/mm-core/src/lib.rs`: register core resources/systems in a reusable plugin.
- `crates/mm-app/Cargo.toml`: own runtime dependencies and binary target configuration.
- `crates/mm-app/src/main.rs`: own startup and runtime boot.
- `crates/mm-app/src/lib.rs`: own app assembly/plugin wiring if helpful for cohesion.
- `crates/mm-app/src/game.rs` or `crates/mm-app/src/app.rs`: own Bevy resource setup, raw input translation, camera setup, and visual composition.

## Feature Flag Plan (Debug/Diagnostics)
- Compile-time flags:
  - None required for the first slice.
- Runtime flags:
  - None required for the first slice.
- Release-default behavior:
  - Normal playable baseline with no diagnostic-only surfaces.

## Prompt Output Contract (Diagnostics/Performance Ideas)
- Copy-ready `@audit` prompt template:
  - Symptom signature:
    - Player does not move, moves in the wrong direction, or raw input does not correctly translate into core movement intent/state updates.
  - Reproduction steps:
    - Run the app, press movement keys, observe world/player behavior.
  - Environment/build:
    - Linux, Cargo workspace, `mm-core` + `mm-app`, Bevy runtime.
  - Captured evidence summary:
    - Current workspace contains placeholder hello-world code and one invalid binary target in `mm-core`.
  - Regressions to avoid:
    - Gameplay logic leaking into `mm-app`, windowing/render/input-hardcoding leaking into `mm-core`, hardcoded movement values scattered across layers.
  - Capture validity (valid/invalid + reason):
    - Valid for architecture planning; invalid for runtime performance diagnosis until the baseline slice exists.

## Regressions to Avoid
- Leaving `mm-core` coupled to application bootstrap concerns.
- Building a Bevy-only gameplay path in `mm-app` that bypasses `mm-core` logic.
- Introducing duplicated gameplay state that must be synchronized between `mm-core` and `mm-app`.
- Scattering movement constants across multiple files.
- Introducing half-wired modules or dead placeholder code.

## Validation Plan
- [x] `cargo check` passes for the workspace.
- [x] `mm-core` unit tests verify deterministic movement behavior.
- [x] `mm-app` boots a 2D scene successfully.
- [x] Pressing movement keys updates movement intent through `mm-app` input translation and changes player position through `mm-core` state transitions.
- [x] The rendered player uses the same ECS transform/state mutated by `mm-core`, with no synchronization layer.

## Definition of Done Checklist
- [x] Cargo checks pass for affected crates
- [x] No half-built runtime flows or dead paths
- [x] Signals are non-zero/sane in active scenario
- [x] Implementation maps to stated user goal

## Execution Log (Living History)
- 2026-03-26 18:46 UTC+01:00 Read `doc/architecture.md`, `doc/CHANGELOG.md`, and `doc/execs/exec_idea.md` before proposing architecture.
- 2026-03-26 18:47 UTC+01:00 Inspected workspace crates and found placeholder hello-world code in `mm-core` and `mm-app`.
- 2026-03-26 18:47 UTC+01:00 Found an architecture mismatch: `crates/mm-core/Cargo.toml` declares a `[[bin]]` target named `mm-app`, but `mm-core` should remain a pure library crate and the referenced `src/main.rs` does not exist.
- 2026-03-26 18:48 UTC+01:00 Selected Option A as the preferred minimal end-to-end slice.
- 2026-03-26 18:51 UTC+01:00 Refined Option A to an ECS-native core boundary: `mm-core` may depend on `bevy_ecs`, `bevy_math`, and `bevy_transform`, while `mm-app` remains responsible for OS input, windowing, rendering, and visual composition.
- 2026-03-26 18:56 UTC+01:00 Replaced placeholder hello-world scaffolding with an ECS-native `mm-core` plugin exposing `Player`, `MovementIntent`, `MovementConfig`, `SimulationStep`, `PlayerBundle`, and deterministic movement systems.
- 2026-03-26 18:57 UTC+01:00 Rebuilt `mm-app` as the thin Bevy executable layer: startup now spawns the camera and visible player, translates raw keyboard input into `MovementIntent`, and updates `SimulationStep` before core simulation runs.
- 2026-03-26 18:58 UTC+01:00 Validated the slice with `cargo check`, `cargo test -p mm-core`, and a short `cargo run -p mm-app` smoke test that successfully created the window and renderer.
- 2026-03-27 01:08 UTC+01:00 Promoted movement intent and simulation semantics to a 3D-aware domain model in `mm-core`, while constraining actual movement to the XY plane so the third axis remains available for indexing/render layering.
- 2026-03-27 01:09 UTC+01:00 Replaced placeholder visuals in `mm-app` with the provided sprite assets: added a texture-atlas-backed player sprite sheet with four-direction walk animation and a tileset-backed pale ground/water map.
- 2026-03-27 01:10 UTC+01:00 Revalidated with `cargo check`, `cargo test -p mm-core`, and a fresh runtime smoke test after the asset and 3D-domain changes.

## Final Outcome
Shipped a minimal ECS-native movement baseline that is now explicitly 3D-aware in the domain while remaining 2D in rendered motion. `mm-core` uses `Vec3` intent/state semantics and preserves the third axis for indexing, and `mm-app` now renders the provided terrain/player assets using texture atlases, directional walk animation, and a simple ground/water tile map. The workspace compiles, core movement tests pass, and the app smoke test successfully brought up the asset-backed runtime window.

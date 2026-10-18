# Architecture

**Target architecture for the GPU transition, adopted in prompt 00.** The checkout still runs the
CPU-authoritative Bevy baseline described under [current baseline](#current-baseline), and
[status](product/status.md) owns what is implemented and verified. Everything else on this page
is the target. Package names are the proposed implementation map. 01 created `world-model` and
`tools/evidence`; 02 created `world-runtime` and `gpu-world`, and 05 `world-store`, each with its
consumers. The others do not exist yet and are created by the stage that first uses them. Where no consumer justifies a
crate, a module is used instead and recorded here and in
[ADR 0002](decisions/0002-native-composition.md).

## One physical world, several consumers

The GPU owns resident cells, body state and their physical updates. Rendering consumes that state
on the same device. The CPU owns intent, residency policy and durable I/O, receiving only selected
observations or explicit snapshots. A saved chunk is an immutable revision, not another mutable
copy of the live chunk.

| Responsibility | Owner | What does not belong there |
| --- | --- | --- |
| Cell meanings, material identities, coordinates, units, command/observation values | `world-model` | Bevy/wgpu types, threads, database transactions. |
| Fixed-tick admission, gameplay intent, residency/checkpoint state machines, bounded worker policy | `world-runtime` | Per-cell CPU simulation, shader dispatch details, redb access. |
| GPU allocations, layouts, physical kernels, command execution, readback pools and submission lifetimes | `gpu-world` | Window/input policy, save durability decisions, ECS entity identities. |
| Rendering passes and derived visual data | `world-render` | Physical mutation, CPU terrain replicas, a second GPU device. |
| Versioned saved-world encoding and redb transactions | `world-store` | Live-world decisions, regeneration on an I/O error, renderer state. |
| Window/input/UI, asset loading and concrete composition | `morbid-app` | Duplicate physics, storage protocols or authority checks. |
| Scenario execution, CPU test references, contract checks and benchmark reports | `monarchy-evidence` | Production fallback simulation or test-only shortcuts in normal execution. |

Ownership does not require every named algorithm to become a crate. Material flows, thermal rules,
body contacts and terrain displacement are modules under the GPU physical owner. They share memory
and an ordered physical tick; separating them behind per-cell traits would obscure the real work.

## Current baseline

This is the source at `a9c1539` with the changes of 01–04; the sprint
[source audit](development/virtual-office/gpu-transition/source-audit.md) lists the symbols. Each
row describes current behavior that the target replaces, except the rows naming `world-model`,
`world-runtime` and `gpu-world`, which are already the target owners. None of it is a second
supported path: the GPU owner executes commands, transfers (02), the material phases (03) and
bodies (04) headlessly, and the baseline application does not use it.

| Responsibility | Current owner and behavior | Target owner |
| --- | --- | --- |
| Cell meaning, materials, coordinates, units, supported settings, generation | `world-model` since 01 (`cell`, `material`, `spatial`, `units`, `config`, `body`, `generation`; `rules` since 03: the automaton's switches, cadences, constants, wake levels and keyed hashes; since 04 `body` also holds the sphere tuning, the substep and apportionment rules, and `command` the rays and targets). The engine, application, GPU owner (through generated declarations) and evidence runner import it; none keeps its own copy. Commands and observations exist with the runtime and the evidence driver as their consumers | `world-model` |
| Cell simulation | `monarch-engine` `core/simulation*`: a CPU Rayon pass over the double-buffered `ActiveWorldGrid`, on a fixed clock inside Bevy `Update`, reading its constants and hashes from `world_model::rules` since 03. The GPU owner runs the same rules since 03, unused by the application | `gpu-world` |
| Bodies | `core/entities/spherical.rs`: ECS `Transform` and `DynamicRigidSphere` hold the physical state; substeps divide frame time; `GridPhysicsApi` mutates the CPU grid; its constants come from `world_model::body::tuning` since 04. The GPU owner runs bodies since 04 (state, substeps, bins, solver, terrain and liquid coupling), unused by the application | `gpu-world`; ECS keeps identity and a `BodyHandle` |
| Observer, brushes, picking, statistics | `core/entities/observer.rs` and `morbid-app` `runtime/{input,dev_tools}` read and write the CPU grid directly | Intent in `world-runtime`; queries, edits and reductions in `gpu-world`; UI in `morbid-app` |
| Residency | Baseline: `spatial-lib` `chunk/manager.rs` and toroidal `grid.rs` over `world_model::spatial` keys; payloads kept in an active map and an LRU cache. Since 05 the target owners exist, unused by the baseline: `world_runtime::residency` (metadata only: phases, request generations, newest revisions, byte budgets) over `gpu-world` slots and snapshots | `world-runtime` policy; GPU slots in `gpu-world` |
| Persistence | Baseline: `morbid-app` `runtime/persistence.rs` over `world_store::legacy` (since 05 the one owner of the old writer's bitcode rows and table; an unreadable row is no longer generated over). Since 05 the target: `world-store` (versioned rows, immutable revisions, atomic checkpoints, the legacy import) behind `world_runtime::store`, with `world_runtime::{checkpoint, session}`; the baseline does not use it | `world-store` |
| Rendering | `morbid-app` `runtime/render.rs`: four Bevy materials, a whole-grid `ShaderBuffer` copy, shaders under `crates/morbid-app/assets` | `world-render` |
| Composition | `morbid_app::run`: Bevy `DefaultPlugins`, `bevy_egui`, Bevy task pools | `morbid-app` native composition |
| Runtime orchestration | `world-runtime` since 02: identities, the `PhysicalWorld` port, bounded receipts, the outcome ledger, the fixed clock and a standalone `bevy_ecs` runtime; since 04 the body monitor (sample selection and lossless body events). Its consumers are the GPU suites and `resident-world` now and the native client from 06; the baseline keeps its own Bevy schedule | `world-runtime` |
| GPU execution | `gpu-world` since 02: one device context and submission timeline, checked resource plans, generated layouts, assembled WGSL, chunk slots, ordered brush edits and column queries, bounded staging, loss and stop handling. Since 03 the material phases with sparse tile scheduling, configuration and totals commands, and per-tick accounting; since 04 bodies (`bodies`: the slot table; `shaders/body`: integration, bins, the pair solver, contact, deformation, displacement, observation), GPU-resolved rays and every body command | `gpu-world` |
| Evidence | `monarchy-evidence` (01–03): docs/static/CPU lanes, shader validation, adapter inspection, the `transfers` and `cells` GPU suites, the CPU reference of the cell rules, the frozen scenarios and the baseline driver; tests and suite cases registered in `coverage.toml` | `monarchy-evidence` |

Cells, bodies and the observer are unordered `Update` systems. The only thing that serializes them
is that each takes `ResMut<ActiveWorldGrid>`. The [one physical tick](#one-physical-tick) below
replaces that arrangement; documenting the old order is not enough.

## Proposed workspace

```text
apps/morbid/                     package morbid-app; native composition
  src/{main,application,input,ui,assets}.rs
  assets/                       runtime art, fonts, UI and imported assets
crates/world-model/             framework-free semantic values and generation (exists since 01)
  src/{cell,material,rules,spatial,units,config,body,command,observation,generation}.rs
crates/world-runtime/           CPU orchestration and consumer-owned ports (exists since 02)
  src/{ids,port,receipts,ledger,clock,runtime}.rs; residency, checkpoint and workers from 05
adapters/gpu-world/             GPU execution of the physical world (exists since 02)
  src/{context,plan,layout,shader,resources,staging,slots,commands,readback,world,faults}.rs
  shaders/{shared,world,sim,faults}/; the material kernels (sim/) since 03, body kernels from 04
  (contracts run on a device as `cargo evidence test gpu`, not as cargo tests: CI has no GPU)
adapters/world-store/           saved-world format and redb mechanism
  src/{format,store,legacy}.rs
  tests/fixtures/               small supported-format and legacy byte fixtures
crates/world-render/            native rendering of GPU-owned state
  src/{renderer,terrain,bodies,lighting,visibility}.rs
  shaders/
tools/evidence/                 Rust-only development leaf
  src/{main,checks,scenario,suites,measurement}.rs; src/bin/gpu-suite.rs runs one GPU case
  scenarios/                    versioned input/configuration documents
  tests/                        runner/parser and evidence-policy checks
docs/                          maintained explanations and active assignments
```

The listed modules communicate intended cohesion; they are not a file-count quota. Introduce each
package only when its first real consumer adopts it. Reuse existing sound code before moving it.
Generic spatial math from `spatial-lib` can move into `world-model::spatial`; its CPU-grid-dependent
chunk manager must be replaced by the residency policy, not renamed as a GPU abstraction, and its
redb backend belongs to `world-store`. Keep a separate spatial crate only if 01 demonstrates an
independent consumer and a meaningful boundary.

### Dependency direction

`A -> B` means A imports B, not that data flows only in that direction.

```text
world-runtime -> world-model
world-runtime -> bevy_ecs (standalone: default features off, `std` only)
gpu-world     -> world-runtime (execution port), world-model, wgpu, naga (offline validation)
world-store   -> world-runtime (storage port), world-model, redb/codec
world-render  -> gpu-world (read-only presentation view), world-model, wgpu
morbid-app    -> world-runtime, gpu-world, world-store, world-render, window/UI
monarchy-evidence -> the owners under test (never the reverse)
```

A GPU boundary may expose wgpu types: GPU memory is precisely its subject. Do not wrap every wgpu
handle to appear mechanism-neutral. The runtime's intent, residency and persistence ports must not
expose those types. Ports operate on batches, tickets and snapshots, not virtual calls per cell.
The GPU adapter's runtime dependency may bring the ECS library transitively; GPU kernels and
adapter interfaces still must not depend on ECS entities or scheduling behavior. `check static`
enforces these directions on each package's own resolved closure (`cargo tree -p`), where the
standalone ECS crates are the only `bevy*` packages allowed.

`world-render` reads the GPU adapter's view; `gpu-world` never imports the renderer. The application
constructs them with one context. The context owns one submission timeline. Compute and render
encoders may be separate, but their submission order and resource lifetimes are not independent.

## State and identity

CPU ECS entities hold logical game identity, configuration and a generation-bearing `BodyHandle`.
The GPU holds positions, velocities, physical flags and solver state. A selected CPU observation
includes its world/session generation and tick; it is not a writable authoritative transform.

World keys are absolute discrete identities. GPU coordinates are local to a declared origin, with
explicit conversion to the renderer's axes. Preserve the current horizontal-cell/elevation-scale
meaning until a tested conversion decision changes it. Do not encode large absolute coordinates
as float shader uniforms or use a toroidal slot as a permanent world key.

Keep logical epoch, submitted tick, GPU-completed tick, observed tick, chunk revision and durable
checkpoint distinct. They answer different questions. A tick encoded for later execution is not
already completed, and an observed edit is not already on disk.

## One physical tick

The runtime owns fixed-step admission and a bounded catch-up policy. Cells and bodies participate
in one documented tick schedule; body substeps subdivide that fixed interval, not arbitrary frame
time. Input is converted into tick commands before physical updates.

The GPU owner orders command application, material transfers/reactions, body integration/contact,
shared deformation/displacement, activity propagation and observation production. The exact order,
read versions and substep coupling are specified in the [GPU contract](reference/gpu-contract.md).
A phase may read an earlier immutable cell view while producing a later version, but this must be
intentional. CPU ECS order does not establish cross-dispatch memory dependencies.

Rendering consumes the ordered physical output without waiting for the CPU to observe completion.
Presentation interpolation or prediction is permitted only as derived visual state. It cannot
write back into physical authority or be used as a secretly different collision model.

## Residency and persistence

The runtime decides which world keys should be resident and how much loading/saving work may be
admitted. The GPU adapter executes upload, activation, freeze/snapshot and slot-retirement commands.
The storage adapter commits immutable versioned payloads. Worker results return through bounded
queues and are accepted only if world identity, request generation and revision still match.

A slot is not reusable merely because its chunk left the camera window. Required outgoing state
must first be captured independently, and all earlier GPU uses must be completed or ordered safely
before reuse. A storage failure retains the payload and prevents false durable acknowledgement.

The [world format](reference/world-format.md) owns checkpoint consistency, format compatibility,
missing-versus-corrupt reads and the reload decision. Retained CPU snapshots are bounded by bytes
and obligations. Normal residency metadata does not contain a permanent full cell array.

Bodies crossing the resident boundary must have an explicit policy: request/pin needed chunks,
clamp supported travel, or suspend/snapshot bodies with the region. They must not disappear or
collide with an unrelated recycled slot. Prompt 04 chooses the body boundary policy; 05 completes
its storage lifecycle. Unloaded regions do not secretly continue simulation in this sprint.

## Assets and shaders

Author simulation shaders beside `gpu-world`; rendering shaders beside `world-render`. Shared
packed-layout declarations have one owner in `gpu-world`, exported by an explicit source assembly
or generated declaration step. This is build-time dependency, not a global shader namespace.

Keep runtime art, UI assets and their provenance in `apps/morbid/assets/`; authoring inputs, when
retained, belong in clearly separate `assets-src/` directories. GPU-generated normals, visibility
lists, body instances and other derived data belong to the renderer, not disk assets or physical
state. Fixtures belong beside their tests. Generated shader assembly, captures, caches and large
logs belong under ignored `target/` or retained external artifacts.

The packaged application must locate its assets independently of the developer's current working
directory. Keep licensing/provenance with imported assets. Do not duplicate Bevy material imports
inside a new string preprocessor and call the engine dependency removed.

## Composition and failure

`morbid-app` parses strict configuration and constructs validated owner-specific plans, one GPU
context, runtime, storage workers and renderer. Device limits are checked against the complete
allocation plan before work is admitted. Headless scenarios construct the same physical owner
without a surface; they do not construct an imitation simulator.

Handle surface resize/loss separately from device loss. A lost device invalidates every live GPU
handle in that session. Stop admission and report the latest durable checkpoint; do not claim
unsnapshotted GPU state was recovered. Checkpoint reconstruction may create a new session epoch,
with stale callbacks and command results rejected.

Shutdown closes new commands, reaches a defined tick boundary, captures required dirty state,
joins/finishes owned work, and reports save success or failure. A normal interactive frame must
not block on a readback or database write. Explicit maintenance/shutdown may wait with bounded
progress/error reporting; those waits are not hidden in performance results.

## Migration boundaries

The original Bevy app is a temporary baseline through 05. GPU stages are independently exercised
headlessly; a temporary visual bridge is optional and must share the same device if used. Never
create two devices and copy the world between them for a demo.

06 makes the native client the sole normal application and removes engine-specific plugins,
materials, task pools and the CPU authority path. 07 verifies dependency/removal and qualification.
Small CPU reference algorithms can remain in the development evidence leaf; the old application
cannot remain behind a fallback flag. Historical source remains in Git or a baseline artifact.

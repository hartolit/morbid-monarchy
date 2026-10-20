# Architecture

**Architecture of the GPU transition, adopted in prompt 00 and implemented through 06.** Every
owner below exists with its consumers: 01 created `world-model` and `tools/evidence`; 02
`world-runtime` and `gpu-world`; 05 `world-store`; 06 `world-render` and the native client
`morbid-app` (`apps/morbid`), and removed the Bevy baseline, its CPU engine and its chunk
manager. [Status](product/status.md) owns what is verified, and 07 qualifies the whole. Where no
consumer justifies a crate, a module is used instead and recorded here and in
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
| Window/input/UI, configuration and concrete composition | `morbid-app` | Duplicate physics, storage protocols or authority checks. |
| Scenario execution, CPU test references, contract checks and benchmark reports | `monarchy-evidence` | Production fallback simulation or test-only shortcuts in normal execution. |

Ownership does not require every named algorithm to become a crate. Material flows, thermal rules,
body contacts and terrain displacement are modules under the GPU physical owner. They share memory
and an ordered physical tick; separating them behind per-cell traits would obscure the real work.

## Current implementation

Since 06 the owners above are the only implementation; the Bevy baseline (its application,
`monarch-engine` and `spatial-lib`) was deleted, and `9a3230c` is the last commit that builds it.
The sprint [source audit](development/virtual-office/gpu-transition/source-audit.md) lists the
baseline's symbols and where each responsibility went.

| Responsibility | Owner and behavior |
| --- | --- |
| Cell meaning, materials, coordinates, units, supported settings, generation | `world-model` (`cell`, `material`, `rules`, `spatial`, `units`, `config`, `body`, `command`, `observation`, `identity`, `resume`, `generation`). The GPU owner reads it through generated declarations; nothing keeps its own copy. |
| Cell simulation | `gpu-world`: the material phases (propose, resolve, react, commit) over the tiles its activity scheduling selects, on the runtime's fixed tick (03). |
| Bodies | `gpu-world`: slot table, device state, substeps of the tick, hashed bins, the pair solver, terrain and liquid coupling, rays and body commands (04). The CPU holds `BodyHandle`s and the runtime's body monitor its observations. |
| Observer, tools, picking, statistics | `morbid-app` `play`: input becomes runtime commands at GPU-resolved rays and column queries; statistics are GPU totals reductions. No CPU terrain read. |
| Residency and persistence | `world_runtime::{residency, checkpoint, session}` (metadata, snapshots at a completed tick, publication) over `gpu-world` slots and snapshots and `world-store` (format version 1, atomic checkpoints, the legacy import) (05). |
| Rendering | `world-render`: landscape layers and spheres drawn from the GPU owner's buffers on its device and timeline, GPU culling, camera follow on the device, lighting, fog, bloom and tone mapping (06). |
| Composition | `morbid-app` (`apps/morbid`, executable `morbid`): a winit window with an explicitly pumped loop, an egui overlay, the world catalog and settings, explicit adapter choice, `WorldSession` over `world-store`, the GPU owner opened presenting to the window, and the renderer (06). |
| Runtime orchestration | `world-runtime`: identities, the `PhysicalWorld` port, receipts, the ledger and body monitor, the fixed clock, workers and the session, in a standalone `bevy_ecs` world. |
| Evidence | `monarchy-evidence`: the lanes, the GPU suites (`transfers`, `cells`, `bodies`, `streaming`, `render`), `resident-world` headless or in the native window, scripted runs of the real client and the native benchmark; the CPU reference of the cell rules stays here as a test oracle. |

The [one physical tick](#one-physical-tick) below is how cells, bodies and commands are ordered;
the baseline's unordered Bevy `Update` systems are gone.

## Workspace

```text
apps/morbid/                    package morbid-app: the native client (executable `morbid`)
  src/{main,lib,options,catalog,live,present,client,input,observer,play,ui,driver,viewer}.rs
crates/world-model/             framework-free semantic values and generation (01)
  src/{cell,material,rules,spatial,units,config,body,command,observation,identity,resume,generation}.rs
crates/world-runtime/           CPU orchestration and consumer-owned ports (02)
  src/{ids,port,receipts,ledger,bodies,clock,runtime,store,workers,residency,checkpoint,session}.rs
crates/world-render/            native rendering of the GPU-owned state (06)
  src/{frame,lighting,geometry,shaders,renderer}.rs
  shaders/                      scene, camera, visibility and post passes
adapters/gpu-world/             GPU execution of the physical world (02)
  src/{context,plan,layout,shader,resources,staging,slots,snapshots,commands,readback,bodies,world,faults}.rs
  shaders/                      shared declarations, world commands, material and body kernels
  (contracts run on a device as `cargo evidence test gpu`, not as cargo tests: CI has no GPU)
adapters/world-store/           saved-world format and redb mechanism (05)
  src/{format,store,legacy,faults}.rs; src/bin/world-store.rs
  tests/fixtures/               the old writer's byte fixtures
tools/evidence/                 Rust-only development leaf
  src/{cli,checks,tests_cpu,gpu,suites,scenario,measurement,bench,native}.rs
  src/bin/{gpu-suite,native-client}.rs
  scenarios/                    frozen measurement workloads
  baselines/                    retained reference frames and baseline measurements
  tests/                        runner/parser and evidence-policy checks
docs/                          maintained explanations and active assignments
```

The client has no asset directory: its shaders are compiled into `world-render`, its fonts are
egui's bundled defaults, and it reads nothing relative to the working directory. Generic spatial
math moved from `spatial-lib` into `world_model::spatial` (01), its storage into
`world_store::legacy` (05); its CPU chunk manager was replaced by the residency owner and deleted
in 06.

### Dependency direction

`A -> B` means A imports B, not that data flows only in that direction.

```text
world-runtime -> world-model
world-runtime -> bevy_ecs (standalone: default features off, `std` only)
gpu-world     -> world-runtime (execution port), world-model, wgpu, naga (offline validation)
world-store   -> world-runtime (storage port), world-model, redb/codec
world-render  -> gpu-world (read-only presentation view), world-runtime (identities and the
                 resident window), world-model, wgpu
morbid-app    -> world-runtime, gpu-world, world-store, world-render, winit, egui
monarchy-evidence -> the owners under test, including morbid-app (never the reverse)
```

A GPU boundary may expose wgpu types: GPU memory is precisely its subject. Do not wrap every wgpu
handle to appear mechanism-neutral. The runtime's intent, residency and persistence ports must not
expose those types. Ports operate on batches, tickets and snapshots, not virtual calls per cell.
The GPU adapter's runtime dependency may bring the ECS library transitively; GPU kernels and
adapter interfaces still must not depend on ECS entities or scheduling behavior. `check static`
enforces these directions on each package's own resolved closure (`cargo tree -p`), where the
standalone ECS crates are the only `bevy*` packages allowed; since 06 that includes the native
client's closure and the evidence runner's.

`world-render` reads the GPU adapter's view; `gpu-world` never imports the renderer. The application
constructs them with one context. The context owns one submission timeline. Compute and render
encoders may be separate, but their submission order and resource lifetimes are not independent:
since 06 presentation frames go through `GpuWorld::submit_presentation` on that timeline, bounded
to two in flight apart from the world's own submissions.

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

Author simulation shaders beside `gpu-world`; rendering shaders beside `world-render` (06). Shared
packed-layout declarations have one owner in `gpu-world`, exported by an explicit source assembly
or generated declaration step. This is build-time dependency, not a global shader namespace.

Runtime art, UI assets and their provenance belong in `apps/morbid/assets/` when the client first
has any (06 needed none); authoring inputs, when retained, belong in clearly separate
`assets-src/` directories. GPU-generated normals, visibility
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

The original Bevy app was a temporary baseline through 05; GPU stages were exercised headlessly,
with no visual bridge. 06 made the native client the sole application and removed the
engine-specific plugins, materials, task pools and the CPU authority path, with no fallback flag.
07 verifies dependency/removal and qualification. The CPU reference of the cell rules remains in
the development evidence leaf as a test oracle. Historical source remains in Git (`9a3230c` is
the last commit with the baseline) and in the retained baseline artifacts under
`tools/evidence/baselines/`.

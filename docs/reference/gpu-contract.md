# CPU–GPU execution contract

**Implementation target.** This document owns semantic obligations, not a frozen Rust API signature.
02 creates the actual typed interfaces and executable layout tests. Change this account with the
implementation; do not maintain a parallel prose-only protocol.

## Identity and time

A live session has a `WorldId` and `SessionEpoch`. Commands carry a session-scoped `CommandId`, an
ordered sequence, intended tick policy, and validated payload. Body references carry index plus
generation; chunk transfers carry absolute key, slot generation, revision and request ticket.
Do not put a raw ECS entity ID into the GPU contract.

Use one monotonically advancing logical tick with explicit rollover behavior. A CPU `u64` can be
encoded as two words; native shader `u64` is not required. Distinguish accepted, encoded, submitted,
GPU-completed, CPU-observed and durable outcomes. A queue submission is not a save acknowledgement.

Normal command retries with the same ID and payload cannot apply twice within the live session;
reuse with different payload is rejected. Keep retention bounded: an acknowledged/expired ID has
an explicit floor or generation rule, not indefinite receipts. A restart creates a new session;
this sprint does not promise replay of all unsaved commands after a crash.

## Commands

The runtime validates semantics/capacity and supplies bounded batches. The GPU adapter validates
resource identity and performs the physical action on its ordered timeline.

| Family | Required meaning |
| --- | --- |
| Edit material | Brush/ray, radius, strength, material/mode and request ID. Resolve the hit against live GPU state and return applied/no-hit/refused with tick. |
| Body control | Spawn validated physical description; apply tick intent/impulse; possess/control selection; attract/lift; remove by generation-bearing handle. No CPU authoritative transform loop. |
| Configuration | Change physical flags, ambient temperature or supported numerical settings at a specified boundary, with required reactivation. |
| Residency | Upload a revision into a reserved slot, activate only when valid, freeze/capture outgoing state, retire safely. |
| Observation | Select a bounded set of bodies/queries/diagnostics and cadence; it is not a request for a permanent full-grid replica. |
| Maintenance | Resize, checkpoint, orderly stop and explicit recovery. These have different admission/wait rules from an ordinary frame. |

Order repeated edits deterministically. A large brush is not thousands of independent unbounded
CPU commands. Batch/rasterize it on the GPU with a single writer per affected cell and a bounded
work domain. Cursor preview may be provisional; the applied result references the actual ray,
tick and world epoch. Do not use a stale CPU hit to claim authoritative placement.

Admission reserves command/result storage before an accepted operation can consume it. On pressure,
return a retryable not-admitted result or apply declared backpressure. Once accepted, preserve its
outcome obligations. Coalescing continuous input is allowed before admission; coalescing must not
silently delete an already accepted discrete edit.

## Cell layout and authority

Start with the current semantic 64-bit column, represented on the GPU as two `u32` words. Its
fields are recorded in [world format](world-format.md). Keep generation/residency, activity,
proposals and render-derived data separate when they have different lifetimes/access patterns.
Do not expand every field to a persistent 32-bit array without a measured case and full memory
accounting.

A field setter is not an atomic material transaction. One invocation owns the complete output cell
for a phase, including interactions between elevation, granular overflow and liquid reaction.
Shared changes are proposed and resolved before committing the output. CPU tools do not write raw
cell words behind this owner after authority transfer.

## Ordered physical work

This is the initial pass model, to be made concrete and tested in 03–04. Arrows mean producer output
is available to the next consumer, not a CPU wait between phases.

```text
apply ordered commands / activate valid uploads
  -> choose active donors and required read/receive halo
  -> compute granular and liquid proposals from the declared immutable input
  -> resolve receiver capacity and accepted transfers
  -> write per-cell material/reaction result to scratch
  -> commit affected cell results
  -> for each fixed body substep:
       integrate/predict -> spatial bins -> contact response
       -> resolve shared deformation/displacement -> commit body/material result
  -> finalize activity, chunk revisions, diagnostics and selected observations
  -> publish the completed logical tick for rendering/snapshot ordering
```

Preserve the baseline's useful local order: granular changes can affect a cell before erosion;
thermal sees that cell's post-flow content while its neighbour survey reads the declared earlier
cell view. Do not silently switch all neighbour reads to a later phase and still claim exact
legacy parity. 03 may simplify this order only with a recorded rule change and independent tests.
Body contact must account for changes committed by preceding body substeps; define which effects
are applied in the current substep and which influence the next, then test the coupling.

Each transfer uses an explicit accepted amount at both ends. Receiver capacity covers all committed
changes sharing that representation; neither a material conversion nor granular compression may
spend the same capacity twice. Donors retain rejected units. Conservation diagnostics classify
intentional sources/sinks and check the total with overflow-safe arithmetic.

Preflight bounded output/work capacity before dependent mutation. Prefer exact bounded batches to
silently truncated lists. If an unexpected mid-tick fault occurs after in-place changes, mark the
session faulted/that tick unfinished; do not render/save it as a completed physical result or claim
a rollback that did not exist. The recovery policy can restore the last valid checkpoint with an
explicit loss boundary. Normal supported workloads must not rely on this fault path.

### Activity

Idle cells are neither copied nor visited merely to preserve them. A dense path is useful as the
initial GPU correctness reference. The production sparse path preserves untouched state and
includes every receive/read dependency; it can compute sparse scratch outputs before committing
them to a canonical array. Global temperature/config changes may legitimately reactivate all cells
once. A recurring full-cell wake clear is not sparse execution.

Logical topology is checked before toroidal addressing. A missing/loading neighbour is a closed
simulation boundary for this transition: no flow or body contact may sample the recycled contents
of its slot. Reactivation wakes boundary dependants. A future open/outflow boundary requires its
own accounting, not accidental modulo wrapping.

### Bodies

Bodies use spatial bins with complete candidate coverage for the supported radius/speed range.
Each body has one output writer or an explicitly resolved reduction. A per-body gather/Jacobi
approach is a suitable starting point; 04 records actual iteration/convergence and capacity rules.
Do not require floating atomic accumulation or a small silently truncated per-bin contact list.

Multiple bodies may disturb the same terrain/fluid cells. Resolve those contributions collectively,
respect source/destination capacity, then commit. Body movement, grounded status, liquid immersion,
crater/rut work and displacement are one coupled physical operation, not independent decorative
passes. Terrain-contact interpolation agrees with the rendered physical surface.

01 fixed the physical limits ([below](#supported-physical-envelope-fixed-in-01)) and 04 proves
them: positive radius/mass, finite inputs, supported speed, substep bounds and largest influence
footprint. Crossing an unavailable region is explicitly suspended/clamped or waits through
residency policy; it does not tunnel into an empty default grid.
Persist/suspend nonresident bodies in 05. Generation-bearing handles remain valid across live
state movement or are explicitly invalidated and replaced—not silently recycled.

### Supported physical envelope (fixed in 01)

`world-model` owns these values in code (`config`, `body`, `units`). The GPU runtime handles every
value inside them and refuses a value outside them before it changes the world; a refusal is
visible, never a silent clamp. The baseline column shows where today's CPU application differs.

| Quantity | Supported | Unit | Reason | Baseline |
| --- | --- | --- | --- | --- |
| Tick rate | 10–240, default 60 | ticks/s | The developer UI's range. | Same. |
| Catch-up | 1–16, default 4 | ticks per presented frame | Backlog beyond the cap is discarded and reported. | Same; the scale profile discards 30–69 s of simulated time per run. |
| Body substep | at most 1/120 | s | A tick is split into the fewest equal substeps no longer than this: 2 at 60 Hz, 12 at 10 Hz, 1 at 240 Hz, so at most 12 per tick and 192 per frame. | Substeps divide frame time, not the tick, at most 8 per frame. |
| Gravity | −32 | world units/s² | Current tuning. | Same. |
| Elevation scale | 0.01–5.0, default 0.5 | world units per elevation unit | Physical configuration, applied at a tick boundary and recorded with the configuration in a checkpoint ([world format](world-format.md#existing-packed-cell)). | Written immediately by the UI; not saved. |
| Body radius | 2–8 | world units | The presets use 4. Below 2, contact sampling sees too few cell vertices; above 8, the footprint (up to 2.75 radii, 22 cells) outgrows the bins. | Unchecked. |
| Body relative density | 0.01–25 | × water | 0.01 is the solver's buoyancy floor; 25 exceeds the densest metals. | Floored at 0.01. |
| Body mass | 0.01–1000 | solver mass unit | Only ratios and the rut depth's square-root scaling use mass; it is not a calibrated kilogram. | Unchecked. |
| Body speed | up to 600 | world units/s | Attract's terminal speed against air drag is 350 / 0.6 ≈ 583. At 1/120 s a body at this speed moves 5 units per substep, more than the smallest radius, so 04 subdivides or sweeps fast bodies. | Unbounded. |
| Bodies | up to 4,096 | count | The capacity profile. | Unbounded; all n(n−1)/2 pairs tested every substep. |
| Brush | radius 0–64, strength 1–255 | cells, units per cell | Radius 64 touches at most 12,853 cells per stroke. | Same. |
| Active window | radius 1–32 | chunks per axis | At radius 32 one packed cell array is 132 MiB, beyond a default 128 MiB storage binding, so 02 requests a larger binding or splits the array. | Same range; CPU arrays. |

Queues and working sets use the [performance budgets](performance-budgets.md#structural-gates):
at most 3 outstanding frame/tick submissions, at most 256 KiB of command plus ordinary observation
payload per presented frame, and 64 MiB for the combined CPU upload/readback/save-retry working
set. 02 sizes the command and observation rings from these numbers and reports the occupancy.

Body behavior is accepted against the tolerances the baseline tests already state, recorded per
test in the [coverage registry](../../tools/evidence/coverage.toml). For example: horizontal drift
under 0.05 world units at rest; sinkage at most the rut depth scale plus 0.25; a floating
submerged fraction within 0.05 of the relative density; a grounded share of at least 90% on bumpy
ground; analytic geometry within 1e-6. Integer quantities (cells, volumes, conservation totals)
compare exactly. 04 does not require bitwise agreement with the CPU baseline.

## Runtime ports (proposed for 02)

These are the boundaries `world-runtime` will own. Each takes batches and returns identified
results; none is a virtual call per cell. They are signatures, not code, because nothing consumes
them before 02. 02 creates them with their first consumer and replaces this section with the
actual interface.

```text
PhysicalWorld (implemented by gpu-world)
  submit(tick_batch: &[Command], tick: Tick) -> Result<SubmissionTicket, Refusal>
  poll_observations(into: &mut ObservationBuffer) -> PollOutcome      // never blocks
  upload_chunk(slot: SlotRef, revision: Revision, cells: &[WorldCell]) -> Result<UploadTicket, Refusal>
  capture_chunks(request: CaptureRequest) -> Result<CaptureTicket, Refusal>

ChunkStore (implemented by world-store)
  load(key: ChunkKey, generation: RequestGeneration) -> LoadTicket    // absent / corrupt / unavailable are distinct
  commit(batch: SaveBatch) -> SaveTicket                              // payload retained until acknowledged
```

`Command`, `BodyObservation`, `BodyHandle`, `ChunkKey` and `WorldCell` already exist in
`world-model`; the baseline driver issues the commands and reports the observations today.
Tickets, refusals, slots and revisions are added by the stages that need them.

## GPU resource lifecycle

The context owns device/queue creation, requested features/limits, submissions and loss reporting.
Resource pools own allocation and high-water accounting. Encoding a render pass must not create a
second device or trigger a CPU physical-state copy. Rendering reads the latest safely ordered view
without requiring CPU completion first.

A resource plan includes allocation and binding sizes, alignment, workgroup/storage/dispatch
limits, slot counts, scratch validity and all temporary replacements. An unsupported resize leaves
the current world intact. A successful resize migrates retained live state GPU-to-GPU where useful;
it does not route the full world through CPU memory merely to change dimensions.

Mapped staging slots follow a concrete state machine:

`free -> reserved -> GPU copy submitted -> completion/map ready -> CPU read/write use -> unmapped -> free`

Late callbacks are checked against their reservation generation and session. Polling is nonblocking
in the normal frame. Bounded submission pressure reduces new admission; it does not accumulate an
unlimited GPU queue. Maintenance waits are explicit and counted separately.

## Observations and reliability

Ordinary readbacks contain selected body samples, command results, query results, aggregated
interaction notifications and diagnostics. They carry source tick and session identity. Rendering
uses GPU body/camera data or explicit presentation prediction; it never waits for these samples to
construct every frame. A CPU gameplay system needing precise immediate contact should move that
physical decision into GPU execution.

Separate transient cosmetic events from gameplay-relevant state. Cosmetic events may be coalesced
or dropped with counters. Important outcomes use reserved command receipts or persistent bounded
per-body/interaction accumulators with acknowledged watermarks; they cannot disappear because an
append buffer filled. Record aggregation semantics so repeated contact does not apply damage twice.
An accumulator's numeric range and handle retirement must also be bounded and tested.

This sprint implements a real CPU consumer for command outcomes and selected contact/immersion
observations, including an overflow/age display. It does not claim the baseline already had a
working damage/audio system. Do not manufacture per-cell “changed” events as the normal bridge.

## Snapshots and completed ticks

A snapshot identifies a completed logical boundary. Reserve independent snapshot storage, order
capture after the requested physical work and before source reuse, then stream out bounded pieces.
The runtime owns when enough payloads are durable to publish a checkpoint; the GPU owner only
reports capture/readback completion. Details are in the world-format contract.

## Minimum contract tests

Execute on real GPU resources: packed-word roundtrip; empty and partial workgroups; repeat/reset;
stale command/body/slot rejection; ordered edit plus query; delayed map completion; zero/full
observation queues; repeated resize; controlled device-loss injection; and resource-limit refusal.
Add family-specific physics and cross-family tests as the sprint progresses. Shader validation or
CPU emulation alone cannot satisfy this boundary.

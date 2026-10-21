# CPU–GPU execution contract

**Implemented in 02 for the command, query and transfer path, in 03 for the material phases, in
04 for bodies, rays and the body observations, in 05 for snapshots and checkpoints and in 06 for
presentation on the same timeline; qualified as a whole in 07.** This document owns the semantic
obligations. The port is `world_runtime::port`, its GPU implementation
`gpu_world::world::GpuWorld`, and the `transfers` (02), `cells` (03), `bodies` (04), `streaming`
(05) and `render` (06) GPU suites and the connected `resident-world` scenario execute this
contract on a device. Change this account with the implementation; do not maintain a parallel
prose-only protocol.

## Identity and time

A live session has a `WorldId` and `SessionEpoch`. Commands carry a session-scoped `CommandId`, an
ordered sequence, intended tick policy, and validated payload. Body references carry index plus
generation; chunk transfers carry absolute key, slot generation, revision and request ticket.
Do not put a raw ECS entity ID into the GPU contract.

Use one monotonically advancing logical tick with explicit rollover behavior. A CPU `u64` can be
encoded as two words; native shader `u64` is not required. Distinguish accepted, encoded, submitted,
GPU-completed, CPU-observed and durable outcomes. A queue submission is not a save acknowledgement.

*Implemented (02).* `world_runtime::ids` defines `WorldId`, `SessionEpoch` (`u32`, so a shader can
echo it), `CommandId`, `Tick` and `ChunkRevision`; wide values travel as a low then a high word.
`Tick::next` and `SessionEpoch::next` refuse to pass their last value instead of wrapping. A tick
is numbered only when it is submitted, so a refused or discarded tick leaves no gap. Receipts
record pending, submitted (with its tick), completed (with its tick and result) and abandoned;
GPU completion of a submission and the CPU decoding its results are reported separately
(`TransferCounters::ticks_submitted` and `ticks_observed`). Nothing is durable in 02.

Normal command retries with the same ID and payload cannot apply twice within the live session;
reuse with different payload is rejected. Keep retention bounded: an acknowledged/expired ID has
an explicit floor or generation rule, not indefinite receipts. A restart creates a new session;
there is no replay of unsaved commands after a crash (no command journal).

*Implemented (02), the floor rule.* `world_runtime::receipts::Receipts` holds a fixed number of
receipts (default 4,096). Issuers choose identities. An identity at or above the floor and not
held is admitted; the same identity with the same command is a `Duplicate` and admits nothing;
with a different command it is a `Conflict`. `acknowledge(through)` releases final receipts up to
`through` and raises the floor past it, so those identities are `Expired`; it is refused while
any of them is unfinished. A full table refuses admission with `Pressure::Receipts`.

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

*Implemented (02).* The live world executes `EditMaterial` (the brush tools, against a centre cell)
and `QueryColumn`; body, attract/lift and ambient-temperature commands were refused at admission
as `NotSupported` (03 implements the climate command, 04 the rest). Edits have no ray yet: the GPU-resolved hit is 04's.
Per tick the runtime takes at most `commands_per_tick` (256) queued commands in admission order;
each has a reserved result record on the device. A stroke runs as one dispatch over its bounding
square, one invocation per cell, so each cell has one writer; strokes of a tick run as separate
dispatches in command order. A stroke that misses the window is not dispatched and its cells are
counted as not resident on the CPU. The edit kernel applies the baseline brush rules
(`world_model` `inject_fluid` for the liquid tools, fire on dry or oiled columns, sand with
compaction into sandstone, raise and lower within the elevation range).

*Implemented (03).* The same path executes `SetAmbientTemperature` and `SetSimulationRules`
(applied by the CPU to the parameters of the tick they arrive in, the last of a kind winning, and
answered `Configured`) and `MeasureTotals` (a reduction over every resident cell in command
order, answered with `WorldTotals`). The edit kernel marks every cell it changes as edited, so
the stroke wakes its reach for the same tick's material phases, and adds what it painted, raised
or removed to the tick's tool counters. The runtime can hold a command until a named tick
(`Runtime::submit_at`), which keeps scripted inputs on their logical tick whatever the frame
pacing.

*Implemented (04).* Every command kind is executed: `SpawnBody`, `RemoveBody`, `DriveBody`,
`JumpBody`, `AttractBodies`, `LiftBodies`, `SetElevationScale` and `QueryRay` join the edits,
queries and configuration. A command acts at a `Target`: a column (its cell vertex) or a
`Ray` (an `f64` origin, a unit direction and a length of at most `RAY_MAX_DISTANCE`, 1000
units). The device resolves a ray (`resolve_target`) against the same triangulated heightfield
bodies collide with, and the nearest live body it crosses; the CPU never reads terrain to place
an edit. A ray-targeted command reads its hit from its own result record; a ray that misses or
reaches a column that is not resident below the highest possible surface is answered
`TargetMissed(SurfaceHit::Miss | NotResident { distance })` and changes nothing. A spawn takes a
slot on the CPU only once the tick is accepted (`BodyCapacity` when the table is full); a body
command naming a removed, never-issued or older-generation handle is answered `StaleBody` (on
the CPU when the table already knows, and checked again on the device), and a spawn whose ray
missed frees its slot when its outcome arrives. Drive accelerates along the drive direction up
to its speed, jump adds the jump impulse along the ground normal only when grounded
(`Jumped { jumped: false }` otherwise), attraction accelerates every live body toward the point
for one tick, and lift raises and centres the bodies within its influence; a suspended body
ignores them.

## Cell layout and authority

Start with the current semantic 64-bit column, represented on the GPU as two `u32` words. Its
fields are recorded in [world format](world-format.md). Keep generation/residency, activity,
proposals and render-derived data separate when they have different lifetimes/access patterns.
Do not expand every field to a persistent 32-bit array without a measured case and full memory
accounting.

*Implemented (02).* Cells are `array<vec2<u32>>`, `.x` holding bits 0–31. The cell field constants
(`CELL_<FIELD>_WORD`, `_SHIFT`, `_MASK`) are generated from `WorldCell::LAYOUT`; no field crosses
the word boundary, and a layout that would is refused at generation. Records shared with kernels
are declared once in `gpu_world::layout` and generate their WGSL structs; every field is one
little-endian 32-bit word, so field `n` is at byte `4n` in both languages:

| Record | Bytes | Use |
| --- | ---: | --- |
| `TickParams` | 128 | Uniform: tick, epoch, command count, window size in chunks, capacities, injected events; since 03 the window's lowest key, the rules running this tick, activity and phase flags, phase thresholds, rule tick and key, parity, tile count and the activity buffer's region offsets. |
| `DispatchParams` | 16 | Uniform, one entry per command position at the uniform offset alignment; entry `i` names command `i`. |
| `CommandRecord` | 112 | Kind, identity, sequence; edit tool, window-local centre, radius, strength; query cell; target kind; body slot and generation; four command parameters; window-local ray origin, direction and length (04). |
| `ResultRecord` | 96 | Identity echo, status, tick, atomic changed / not-resident counts, column words, slot; ten payload words (ray hit, body, target point; 04). |
| `EventHeader`, `EventRecord` | 16, 16 | Cosmetic event buffer: atomic attempt and drop counts, capacity; kind and three words. |
| `ChunkSlotRecord` | 32 | Absolute key, generation, revision, state of each slot. |
| `DiagnosticsRecord` | 144 | Per tick (03): the material phases' 64-bit accounting, tool counters, tiles and cells visited, rejected transfers, stale transfer reads and receive mismatches. |
| `TotalsRecord` | 96 | Per totals command (03): 64-bit fluid volume per fluid material, elevation, loose volume and cells. |
| `BodyState` | 128 | Per body (04): anchor chunk and in-chunk position, velocity, flags, generation, ground normal, submerged fractions, pending wake, the filtered ground plane, the six interaction counters, touching count, liquid, this tick's events and impact, the packed terrain-contact normal. |
| `BodyConfig` | 16 | Per body: mass, radius, relative density, inverse mass. |
| `BodySampleRecord` | 96 | Per selected body and tick: status, the echoed handle, flags, anchored position, velocity, ground normal, submerged fraction, liquid, touching, counters. |
| `BodyParams` | 128 | Uniform per tick: body extent and capacity, substeps and their length, elevation scale, samples, broadphase, hash mask, the regions of the bodies and bins buffers. |
| `BodyDispatch` | 16 | Uniform, one entry per (substep, pass) at the offset alignment: substep, pass, ping-pong parity. |
| `BodyDiagnosticsRecord` | 128 | Per tick: candidate pairs and pair visits (64-bit), contacts, swept contacts, terrain contacts, residual overlap and approach, speed-limited substeps, the body accounting (64-bit), craters, ruts, splashes, wakes, lost claims, live and suspended bodies. |

Kernels see window-local coordinates only: cell `(0, 0)` is the first cell of the window's lowest
chunk. A window map holds one slot index per window chunk, or `SLOT_NONE`, which every kernel treats
as a closed boundary. Goldens in `gpu-world` fix the strides and words by hand; the
`cell-layout-probe`, `record-layout-probe` and `layout-specimens-detected` cases execute the
declarations on the device and prove a shifted field and swapped record fields are detected.

A field setter is not an atomic material transaction. One invocation owns the complete output cell
for a phase, including interactions between elevation, granular overflow and liquid reaction.
Shared changes are proposed and resolved before committing the output. CPU tools do not write raw
cell words behind this owner after authority transfer.

## Ordered physical work

This is the initial pass model, made concrete for cells in 03 ([cell phases](#cell-phases-03)) and
to be made concrete for bodies in 04. Arrows mean producer output
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

*Order of a tick (02, material phases since 03):*

```text
copies: tick parameters, event header reset, command records, slot-record and window-map patches
        (retirements first, then activations), staged chunk cells; clears of the activity
        counters, chunk wake requests, the tick's diagnostics and the tile records of activated
        chunks; the chunk wake requests of this tick's activations and retirements
compute pass: begin_tick (one invocation per result record) -> one dispatch per command, in order
              -> scan -> args -> wake -> args -> propose -> resolve -> react -> commit
copies: results, events, diagnostics and totals (and resolved pass timestamps) -> tick readback;
        requested captures -> capture staging
```

wgpu orders dispatches that use the same read-write storage buffer with a barrier (wgpu-hal 29.0.4
treats no `STORAGE_READ_WRITE` use as ordered), so each command reads the results of the commands
before it, and each material pass reads the finished output of the one before; the copies after
the pass read the finished results. Uploads therefore activate before the tick's commands, and a
capture holds the cells at the end of the tick, after the material phases. Since 04 a fourth
compute pass runs the body substeps after `commit` (below), and a capture holds the cells after
them.

### Cell phases (03)

*Implemented (03).* Specified in checkpoint A before any kernel and implemented as specified by
`gpu_world` (`shaders/sim/`, `shaders/shared/`, `world/tick.rs`); the CPU reference in
`monarchy-evidence` implements the same rules in the baseline's gather form, and the `cells`
suite compares the two tick by tick.

The material phases form one tick of the cellular rules described in
[world format](world-format.md#existing-packed-cell) and owned by `world_model::{cell, material,
rules}`. `S0` is the canonical cell array after the tick's commands; every neighbour read of every
rule reads `S0`, and a cell's own rules apply in the fixed order granular, liquid, thermal,
biology, fire to its own running value, exactly as the baseline does. A pass that consumes another
cell's output of an earlier pass is a separate dispatch: no global dependency is carried by a
workgroup barrier. Workgroup barriers only combine one tile's own results.

Work is scheduled in **tiles** of 8 × 8 cells, 64 per chunk; one workgroup of 64 invocations
processes one tile. Each cell has a **wake level** for the tick (0 asleep, 1 halo, 2–3 active),
with the baseline's meaning: an active cell donates material and runs its reactions, a halo cell
only receives, an asleep cell is not visited. A tile with any awake cell is a **process tile**.

| Pass | Domain | Reads | Writes (one writer per value unless marked atomic) | Consumer |
| --- | --- | --- | --- | --- |
| `apply_edit` (command) | Stroke square | Own cell | Own cell; the tile's edited bit and candidate flags (atomic OR); tool source and sink counters (atomic, 64-bit) | Later commands; `scan`, `wake` |
| `measure_totals` (command) | Every resident cell | Own cell | The command's totals record (atomic, 64-bit) | Tick readback |
| `scan` | Every window tile | Own candidate flag, then clears it; wake requests of the tile's chunk and adjacent chunks; window map | Own tile state; candidate list (atomic append) | `args`, `wake` |
| `args` | One invocation | List counts | Indirect dispatch arguments (separate buffer) | `wake`, material passes |
| `wake` | Candidate tiles | Changed and persist bitmaps of tick n−1 and edited bitmaps of tick n of the 3 × 3 tiles; chunk wake requests | Own wake levels and tile state; process list (atomic append) | Material passes |
| `propose` | Active cells of process tiles | `S0` within 2 cells; own wake | Own proposal (granular and liquid destination and amount) | `resolve`, `react` |
| `resolve` | Awake cells of process tiles | `S0`, proposals and wake levels within 1 cell | Own resolution (winning source and accepted amount per family) | `react` |
| `react` | Awake cells of process tiles | `S0` within 1 cell; own proposal; resolutions within 1 cell; own wake | Own scratch cell; the tile's changed and persist bitmaps for tick n, clearing tick n−1's and the edited bitmap; candidate flags of the 3 × 3 tiles for tick n+1 (atomic OR); diagnostics (atomic, 64-bit) | `commit`; next tick's `scan` and `wake` |
| `commit` | Awake cells of process tiles | Own scratch and canonical cell | Own canonical cell, when it differs | Next tick, captures, renderer |

A donor's decision reads cells two away from it (a receiver's further neighbours for the liquid
trickle), its receiver's arbitration reads proposals one cell away, and its commit reads the
receiver's resolution: the output of a cell depends on `S0` within four cells, as in the
baseline's gather formulation, but every transfer is evaluated once. A proposal carries the donor's
amount; the receiver picks the baseline's winner and accepts at most what it can hold
([accounting](#material-accounting-03)); the donor drains exactly the accepted amount and keeps the
rest. A stale proposal or resolution cannot be consumed: each word carries the low eight bits of
the tick, every read also requires the writer to be awake in this tick, and a mismatch is counted
in the diagnostics (expected zero) and treated as no transfer.

**Activity scheduling.** After `react`, each process tile publishes which cells changed (a value
change, or an active cell kept awake by immature foliage, fire or a pending phase change) and which
cells *persist* (unchanged at level 3). The next tick's `wake` gives each cell of a candidate tile
level 3 when a changed or edited cell lies within 2 cells, level 2 when it persists, and level 1
when a changed or edited cell lies within 3 cells or a persisting one within 1 — the baseline's
`schedule_wake` radii, evaluated as a gather. A tile is a candidate when its own or a neighbouring
tile's changed, persist or edited cells reach it (each writer marks exactly the neighbouring tiles
its radius reaches), when its chunk or an adjacent one has a wake request, or when the tick wakes
everything. Edits mark their changed cells in the tick they run, so the stroke is simulated in the
same tick. An activated chunk counts as edited throughout, which wakes its boundary dependants; a
retired chunk counts as edited for its neighbours. A change of ambient temperature or simulation
rules, and a resize, wake every resident cell once.

`scan` touches one word per window tile and `wake` only candidate tiles; neither touches cells. In
**dense** execution (a maintained test strategy, `GpuWorldConfig::execution`), every resident tile
is a candidate and a process tile, so every cell is visited and committed every tick; the wake
levels, and so the result, are identical by construction, which is what the dense/sparse
comparison checks. Lists and scratch are sized for every tile being active; nothing is appended
beyond a bound, so no list can overflow or truncate. Indirect dispatches split a count into
`x ≤ 65,535` and `y = ⌈count / 65,535⌉` workgroups.

**Clock and cadence.** Ticks are 64-bit, carried as two words. Granular movement runs on odd ticks,
as in the baseline. Biology is sampled once in ten ticks per active cell and fire once in two, by
keyed draws. Nothing reads wall time. The runtime's clock turns frame time into whole ticks; a
frame runs at most the catch-up cap, and the backlog beyond it is discarded and counted
(`Runtime::discarded_ticks`; dropped simulated time is that times the tick duration). A tick the
device cannot take yet (in-flight backpressure) stays owed within the cap. The world at a given
tick does not depend on the presentation rate (`cells` `presentation-rates`).

**Keyed randomness and tie-breaking.** Every rule that needs a pseudo-random choice uses
`world_model::rules`: the baseline's position-and-tick hash for flow shuffles, granular stiction,
erosion and phase rolls, keyed by a rule key derived from the world's rule seed and the tick's
high word, and a keyed draw with a distinct salt per decision for biology and fire. The key of
seed 0 at ticks below 2³² is 0, which makes the deterministic rules reproduce the baseline cell for
cell; the CPU reference in `monarchy-evidence` is checked against the baseline engine on that
basis. The baseline's biology and fire drew from thread-seeded generators; their keyed draws are a
**semantic change**, not a reproduction: the probabilities per decision are the same, the outcomes
are not.

#### Material accounting (03)

Column quantities are integer elevation units: the fluid volume `L` (every liquid and ice share it)
and the solid column `S` (bedrock elevation plus loose granular volume). The one-label column has
no per-species volume, so a mixing rule may change which label a column carries; only `L` and `S`
are conserved quantities.

| Operation | Conserved | Sink, source or conversion (classified counter) | Capacity policy |
| --- | --- | --- | --- |
| Liquid flow | `L` between donor and receiver | — | The receiver accepts at most its free volume as of `S0`; one source per receiver per tick. |
| Magma into a coolant column | `L + S` | Magma becomes stone bedrock (`quench`) | Accepted only up to the elevation room left after the receiver's largest possible granular compaction this tick; the donor keeps the rest. |
| Coolant into a magma column | `L + S` | The resident magma becomes stone (`quench`) | Accepted only when the whole resident magma fits under the ceiling; otherwise refused and left at the donor. |
| Thermal quench | `L + S` | Magma becomes stone (`quench`) | Converts what fits; the rest stays liquid magma, and the cell does not stay awake retrying. |
| Freeze, thaw | `L` | — | Label change. |
| Boiling | — | Coolant beside magma (`boil`, 2 per contact) | Drains at most the resident volume. |
| Combustion | — | Burning oil (`burn`, 1 per burning tick) | Drains at most the resident volume. |
| Plant growth | — | Water or blood drawn by growing foliage (`uptake`, 2 per growth step) | Drains at most the resident volume. |
| Granular flow, erosion, compaction | `S` | — (erosion counted for information) | A receiver accepts at most its structural room; compaction of excess loose material into bedrock stays under the ceiling. |
| Tools | — | Painted liquid (`tool fluid in`), sand, raise and lower (`tool solid in/out`) | Exactly the change the stroke made; a full or capped column adds less than the strength asks for. |

Per tick, the diagnostics hold the sums of every processed cell's increase and decrease of `L` and
`S` and the classified counters, all 64-bit with carry. Exact conservation means
`ΔL + quench + boil + burn + uptake = 0` and `ΔS − quench = 0` for the material phases; tools are
counted separately in the command phase. The runtime's ledger checks both every tick. Whole-world
totals come from the `MeasureTotals` command, a labelled diagnostic reduction over every resident
cell, never from a cell download.

#### Rule changes from the baseline (03)

| Change | Reason and evidence |
| --- | --- |
| Biology and fire use keyed draws, not thread-seeded generators. | Reproducibility; same probabilities per decision. |
| Quench converts only what fits under the elevation ceiling, and coolant reaching a magma column at the ceiling is refused instead of recursing. | The baseline drained magma before a capacity-limited raise and lost the remainder. Fixed at the owner, `WorldCell::quench` and `receive_fluid`, with an independent fixture. |
| A receiver accepts only what it can hold; the donor drains the accepted amount. | The baseline donor drained the full proposal even when the receiver raised less bedrock than that at the ceiling. |
| Changing a simulation rule wakes every resident cell. | The baseline woke only on a temperature change, so a re-enabled rule never resumed in a sleeping region. |
| Immature foliage and fire keep their cell awake only while biology or fire runs. | With the rule off nothing can advance them, yet the baseline kept them awake for ever: generated terrain (immature foliage) never slept under the deterministic rules, a constant 64% of its tiles at radius 6 (found by the `activity-measurement` case). Switching the rule back on wakes every cell. |
| Activated chunks wake themselves and their boundary dependants; retiring a chunk wakes its border. | The baseline never woke a streamed chunk (source audit, found while measuring). |
| Captures hold the cells at the end of the tick. | A completed tick, the boundary a checkpoint needs. |

The injected-magma behavior of the mixed scenario is kept: painting lava takes a column over
(`inject_fluid`), and quenching then turns it into stone, so repeated strokes still raise a
column towards the ceiling, where the remainder now stays liquid instead of disappearing.

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

*Implemented (03).* There is no toroidal addressing on the device: kernels address window-local
cells through the window map, a cell outside the window or in a chunk without a slot is absent to
every rule, and absolute positions (for the hashes only) are the window's lowest key times 64
plus the local position, with wrapping arithmetic. Activation and retirement wake the reach of
the chunk's cells (see [cell phases](#cell-phases-03)); a resize wakes everything once. The
counts and dispatch arguments never leave the device to schedule work: the per-tick diagnostics
that report them arrive in the ordinary asynchronous readback.

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

#### Bodies (04)

*Identity.* A body is a slot of a bounded table (`transfer.bodies`, up to 4,096) and a 32-bit
generation; `BodyHandle` carries both. The CPU table (`gpu_world::bodies::BodyTable`) allocates
the lowest free slot, bumps the generation on release and retires a slot whose generation is
exhausted instead of reusing it; the device keeps the generation in `BodyState` and checks it
for every body command. The kernels run over the extent (highest live slot plus one).

*State.* Body state lives on the device. A position is anchored: the chunk key of an anchor
plus a position inside it, re-anchored at the end of every substep, so a body far from the world
origin keeps its precision and a window move changes nothing. Kernels compute in window-local
render coordinates (x the cell x, z minus the cell y). The CPU holds intent (commands) and
observations (samples), never a pose to write back.

*Clock.* Each `TickBatch` carries the tick rate; the tick is split into
`substeps_per_tick(rate)` equal substeps of at most 1/120 s (2 at 60 Hz, 12 at 10 Hz, 1 at 144
and 240 Hz). A body integrates in the tick of its spawn. The same scheduled commands give
bit-identical bodies at one tick whatever the presentation rate (`body-presentation-rates`).
Speed is capped at 600 units/s and every capped substep is counted; no substep travels more than
5 units.

*Order and read versions.* The body pass follows the tick's material commit. Per substep:

```text
bins_clear -> body_begin (suspend at the residency edge; gravity, buoyancy, drag; the speed cap)
  -> bins_count -> bins_scan -> bins_scatter        (every binned body, at its integrated position)
  -> pairs_count -> position passes -> velocity passes -> pairs_residual
  -> claims_clear -> contact_prepare -> contact_apply -> contact_response   (terrain, round 1)
  -> claims_clear -> displace_prepare -> displace_apply                     (liquid, round 2)
  -> body_end (re-anchor, counters, the tick's cosmetic event)
then once per tick: body_observe (the selected samples)
```

A substep reads the cells as the tick's material phases and the earlier substeps left them:
deformation committed in one substep is the ground the next substep touches. Every cell a body
changes is marked edited, so the next tick's material phases wake it (a crater's walls slump,
displaced water flows back). Bodies read other bodies only through the solver's ping-pong
halves, so no pass reads a value another invocation is writing.

*Broadphase.* A counting sort of every binned body (active, and suspended bodies inside the
window, which are obstacles) into a hash table of 3-D bins, a power of two at least twice the
capacity. The bin edge is the farthest two bodies can be apart and still meet in the substep
(twice the largest live radius and the farthest travel, the contact slop and the solver's
margin), so the 27 bins around a body hold every possible partner; a bin has no capacity, so no
pair is dropped, and hash collisions only add candidates the generation test rejects. Candidate
pairs are generated once per substep from the integrated positions; the all-pairs broadphase (a
`GpuWorldConfig::broadphase` mode kept for comparison) applies the same generation test to every
slot, so both solve exactly the same pairs and give bit-identical bodies (`dense-cluster`).
`broadphase-oracle` checks the bins against an exhaustive bound on crafted clusters, coincident
centres, a lattice on the bin edges and fast bodies.

*Solver.* A per-body Jacobi gather with explicit limits: `GpuWorldConfig::pair_iterations`
position and velocity passes per substep (each even, 2–16, default 16 and 16). Each pass runs one
64-lane workgroup per body; the lanes share its candidates and every contribution meets the
others as an integer (fixed-point sums at 2⁻¹⁶ units and 2⁻¹² units/s, counts) or as the bits of
a non-negative float (maxima), so the result is independent of order and deterministic. Position
passes split an overlap equally between the pair, move a body by the mean of the corrections
still needed, and never move it more than 0.5 units from where it was binned in one substep (a
maximum depenetration speed, which also bounds the margin candidate generation allows). Velocity
passes drive a touching pair toward its parting speed, the baseline restitution (0.2) of the
approach the solve started from, or none below the baseline's settle speed, by impulses split by
inverse mass and scaled by one over the larger contact count of the pair, so impulses stay equal
and opposite. A pair that meets during the substep takes its normal at the time of impact, and
centres that cross keep the side they started on, so fast bodies do not pass through each other
(`fast-bodies`). A body that touched terrain in the last substep slides along it rather than
being pushed into it. Convergence limit, measured: in a dense pile of 512 bodies of radius 2–8,
settled, no pair overlaps by more than 0.4 of its smaller radius at the default passes, and
fewer passes leave it more compressed (`dense-cluster`).

*Terrain contact and deformation.* One workgroup per body. `contact_prepare` sweeps a fast body
in steps of at most one unit against the unyielded surface, fits the ground plane and decides a
crater or a rut; `contact_apply` crushes and shoves the material under its footprint;
`contact_response` pushes the body out and applies the baseline's bounce, sweeps and rolling
friction, and decides grounding; `displace_*` throws splashes and wakes. Several bodies may want
the same cells: every footprint cell is claimed per round with an `atomicMin` of a key ranking
the cell under a body before a deposit and the nearer body before the farther, and a body
writes only cells it owns, treating the others as closed; a lost claim is displacement the body
did not do (`claims_lost`), never lost material. Displaced units are apportioned over the
receiving cells by cumulative rounding with an exact 32-bit multiply-divide, which never exceeds
a cell's room. A brush stroke in the tick's command pass and the body substeps write the same
cells at different points of the timeline, never concurrently (`shared-deformation`).

*Accounting.* Each tick's `BodyReport` (outcome `BodiesCompleted`) carries the liquid and solid
the bodies moved out of and into cells, and the bedrock compacted under craters and trails, the
one declared sink: `fluid_in == fluid_out` and `solid_in == solid_out - compaction` on every
tick, or the ledger counts it inconsistent. Restitution and crushing losses are counted as
absorbed energy, a separate quantity.

*Residency edge.* A body whose reach box (2.75 radii plus 2 plus its travel) touches a chunk
that is not resident, or whose anchor is far outside the window, is suspended: frozen with its
state and identity, binned as an immovable obstacle while inside the window, ignoring controls,
and resumed when the terrain under it is resident again (`residency-edge`). Missing chunks are
never empty space and wrapped slots are never neighbours. 05 persists suspended bodies and
reloads them with their chunks.

*Changes from the baseline (04).* Recorded rule changes, each with a test that would fail
without it: substeps divide the tick, not frame time (`fixed-tick-substeps`,
`body-presentation-rates`); pairs are solved by Jacobi passes, not one sequential pass, with
equal position splits and a 0.5-unit correction bound per substep (`dense-cluster`); a pair's
bounce uses the ground's settle threshold, so resting bodies do not bounce on each other
(`dense-cluster`, `pair-collision`); pairs and terrain are swept, and speed is capped at 600
(`fast-bodies`, `fixed-tick-substeps`); several bodies on the same cells share them by claims
instead of overwriting each other (`shared-deformation`); a body at missing terrain is suspended
instead of meeting an empty grid (`residency-edge`). The baseline's sphere tuning is unchanged in
`world_model::body::tuning`, which the removed CPU engine also read until 06.

*Preflight and faults.* A tick's command, staging, sample and readback reservations are taken
before any command is encoded, and the CPU body table changes only once the tick is accepted,
so a refused tick changes nothing. Device work is bounded by construction: a footprint fits
the workgroup's 27 × 27 cells, a ray marches at most 64 segments of 128 quads, a pass visits a
body's candidates once. A readback whose results, samples or ray answers do not decode (another
handle, an unknown surface outcome, a body slot beyond the table) faults the session; the tick
is not reported completed.

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
| Body speed | up to 600 | world units/s | Attract's terminal speed against air drag is 350 / 0.6 ≈ 583. At 1/120 s a body at this speed moves 5 units per substep, more than the smallest radius; 04 sweeps fast bodies against terrain and each other and caps the speed, counting capped substeps. | Unbounded. |
| Bodies | up to 4,096 | count | The capacity profile; a spawn into a full table is refused with the capacity. | Unbounded; all n(n−1)/2 pairs tested every substep. |
| Ray | up to 1,000 | world units | Bounds the device's traversal: 64 segments, each marching at most 128 quads. | The pointer's ray, unbounded. |
| Brush | radius 0–64, strength 1–255 | cells, units per cell | Radius 64 touches at most 12,853 cells per stroke. | Same. |
| Active window | radius 1–32 | chunks per axis | At radius 32 one packed cell array is 132 MiB, beyond a default 128 MiB storage binding. 02 requests the larger binding; a device that cannot grant it refuses radius 32 ([below](#gpu-resource-lifecycle)). | Same range; CPU arrays. |

Queues and working sets use the [performance budgets](performance-budgets.md#structural-gates):
at most 3 outstanding frame/tick submissions, at most 256 KiB of command plus ordinary observation
payload per presented frame, and 64 MiB for the combined CPU upload/readback/save-retry working
set. 02 sized the command and observation rings from these numbers
([observations](#observations-and-reliability)); `TransferCounters` reports their occupancy.

Body behavior is accepted against the tolerances the baseline tests already state, recorded per
test in the [coverage registry](../../tools/evidence/coverage.toml). For example: horizontal drift
under 0.05 world units at rest; sinkage at most the rut depth scale plus 0.25; a floating
submerged fraction within 0.05 of the relative density; a grounded share of at least 90% on bumpy
ground; analytic geometry within 1e-6. Integer quantities (cells, volumes, conservation totals)
compare exactly. 04 does not require bitwise agreement with the CPU baseline.

## Runtime ports (implemented in 02)

`world_runtime::port::PhysicalWorld` is the port; `gpu_world::world::GpuWorld` implements it, and
the composition root chooses it. Every operation takes a batch or one bounded request and returns
at once with a ticket or a typed `Refusal`; results arrive through `poll`, which never blocks:

```text
PhysicalWorld (world_runtime::port)
  session() / capabilities() / next_tick() / counters()
  submit_tick(&TickBatch { session, tick, commands }) -> Result<SubmissionTicket, Refusal>
  reserve_slot(session, ChunkKey) -> Result<SlotRef, Refusal>
  upload_chunk(SlotRef, ChunkRevision, &[WorldCell; 4096]) -> Result<UploadTicket, Refusal>
  retire_slot(SlotRef) / capture_chunk(SlotRef) -> CaptureTicket / take_capture(ticket, &mut cells)
  resize(&ResizeRequest { session, window }) -> Result<ResizeReport, Refusal>
  poll(&mut Vec<Outcome>) -> PollReport                    // never blocks
  stop(timeout, &mut Vec<Outcome>) -> DrainReport          // the explicit maintenance wait
```

`Refusal` distinguishes retryable `Pressure` (in-flight submissions, each staging pool, slots, slot
changes, the runtime's queue and receipts), stale session or slot, out-of-order or exhausted ticks,
invalid payloads, unsupported command kinds, capacity limits (`CapacityLimit` names the resource,
the request and what is available), stop, loss and fault. A refused request changed nothing.
`Outcome` carries command outcomes, completed-tick reports, upload activations, ready captures,
cosmetic events and their overflow, and abandoned obligations with their reason. Since 03 a
completed-tick report carries the tick's material accounting (`CellReport`: the 64-bit sums, the
classified sinks, the tool counters, the tiles and cells visited, and the stale-read and mismatch
counts that must be zero) and, with timestamps, the device time of each of its three passes.

*Implemented (05).* The port gained the snapshot operations (`snapshot_chunk`, `snapshot_world`,
`take_snapshot`, `take_body_snapshot`), `submit_transfers`, `restore_bodies` and
`preflight_resize` ([snapshots](#snapshots-and-completed-ticks)), the outcomes `Snapshot` and
`BodySnapshot`, the obligations `Snapshot` and `WorldSnapshot`, and `Pressure::SnapshotImage`.
The storage port is `world_runtime::store::WorldStore` ([world format](world-format.md#saved-world-format-version-1-05));
`world_runtime::{residency, checkpoint, session}` are its consumers, and
`gpu_world::world::GpuWorldConfig::resume` opens the live world at a saved checkpoint.

`world_runtime::runtime::Runtime` composes the runtime as a standalone `bevy_ecs` world whose
schedule runs once per frame: `advance_clock -> submit_due_ticks -> poll_world -> consume_outcomes`.
Issuers call `Runtime::submit(CommandId, Command)`, or `submit_at` with the earliest tick a
command may apply in; residency requests go through the runtime too, which reserves room for their
outcomes first. `Runtime::set_clock` changes the tick rate and catch-up cap within their ranges.
The ledger counts every tick whose material accounting does not balance
(`LedgerSummary::inconsistent_ticks`), so the error is visible in ordinary operation. Since 05 the storage port is
`world_runtime::store::WorldStore`, with the residency and checkpoint owners as its consumers.

## GPU resource lifecycle

The context owns device/queue creation, requested features/limits, submissions and loss reporting.
Resource pools own allocation and high-water accounting. Encoding a render pass must not create a
second device or trigger a CPU physical-state copy. Rendering reads the latest safely ordered view
without requiring CPU completion first.

*Implemented (02).* `gpu_world::context::GpuContext` opens only the adapter named by its stable
identifier (a software adapter needs explicit consent) and requests WebGPU's default limits,
raising only the largest buffer and storage binding to what the radius-32 plan needs (a
138,444,800-byte binding; since 05 a 139,034,624-byte buffer, the world snapshot image of 4,096
bodies' records after the cells), capped by the adapter; `TIMESTAMP_QUERY` is requested only when timestamps are wanted and
supported. Every session buffer is created through the context inside error scopes, so a refused
allocation is a typed error and requested bytes and their peak are counted. Submissions go through
its one timeline: at most `max_in_flight` (3) uncompleted submissions, each tracked by serial;
completion is observed from `on_submitted_work_done` without waiting. Pipelines are compiled once
per session from the assembled source; a new device compiles them again. Other consumers on the
same device (the layout probes) read through `GpuWorld::view` and encode on the same timeline with
`submit_external`.

*Presentation (06).* `GpuWorld::open_presenting` opens the session on an instance of the
adapter's backend only and creates the window's surface on it, refusing an adapter that cannot
present to it; a lost surface is created again with `create_surface`, and only the primary
backends are offered for presenting (`presentable_adapters`). The renderer and the overlay
encode a frame through `submit_presentation`, on the same timeline after the frame's ticks, with
their own bound of two uncompleted presentation submissions apart from the world's
`max_in_flight`; `presentation_ready` refuses a frame while both are in flight, and the client
then draws nothing that iteration rather than wait. The renderer reads the cells, the window map
and body state through the view and replaces its bind groups only when the view's buffer
generation changes (a resize); it derives visibility and the follow camera on the device and
never reads physical state back.

The radius-32 decision: the plan requests a larger storage binding. On a device that cannot
grant it (as with WebGPU's default 128 MiB), a window whose cell array exceeds the granted binding
is refused at open and at resize as `CapacityLimit { StorageBinding, requested, available }`; with
equal radii that is radius 32, while radius 31 (130,056,192 bytes) fits. Splitting the array is not
implemented.

A resource plan includes allocation and binding sizes, alignment, workgroup/storage/dispatch
limits, slot counts, scratch validity and all temporary replacements. An unsupported resize leaves
the current world intact. A successful resize migrates retained live state GPU-to-GPU where useful;
it does not route the full world through CPU memory merely to change dimensions.

Mapped staging slots follow a concrete state machine:

`free -> reserved -> GPU copy submitted -> completion/map ready -> CPU read/write use -> unmapped -> free`

Late callbacks are checked against their reservation generation and session. Polling is nonblocking
in the normal frame. Bounded submission pressure reduces new admission; it does not accumulate an
unlimited GPU queue. Maintenance waits are explicit and counted separately.

*Implemented (02).* `gpu_world::staging` keeps four pools of equally sized buffers: tick uploads and
tick readbacks (`max_in_flight + 1` each), chunk uploads and captures (32 each by default, 32 KiB).
Upload buffers start mapped and are remapped after the submission that read them completes;
readback buffers are mapped right after submission and stay mapped until decoded or collected.
Each reservation advances the buffer's generation; a map callback writes only a fixed mailbox entry
for its buffer with that generation, and a delivery whose generation no longer matches is counted
stale. An empty pool refuses with its `Pressure` kind. Chunk slots follow
`free -> reserved -> staged -> activating -> resident -> retire requested -> retiring -> free`;
retiring advances the generation at once, and the slot is reused only after the tick that cleared
it completes. A resize plans the new window, refuses if the old and new window buffers together
exceed the device budget, if a resident chunk would leave the window, or while slot changes or
reservations are pending, then copies retained chunks device-to-device and keeps the old buffers
until that submission completes (`ResizeReport::peak_overlap_bytes`).

Device loss is reported by wgpu's loss callback; wgpu 29.0.4 panics from `Device::poll` when a
backend reports a lost device, so the poll is isolated and turned into the lost state. A lost
session, or one with an uncaptured device error or a result failing its identity check, abandons
every outstanding obligation once and admits nothing; a new session has a later epoch and refuses
every older slot, ticket and tick as `StaleSession`.

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

The runtime implements a real CPU consumer for command outcomes and selected contact/immersion
observations, including an overflow/age display. It does not claim the baseline already had a
working damage/audio system. Do not manufacture per-cell “changed” events as the normal bridge.

*Implemented (02).* `world_runtime::ledger::OutcomeLedger` is that consumer; later producers report
through it. Important outcomes are command results (one reserved receipt and one reserved device
result record each) and residency results (room reserved in the ledger before an upload or capture
is accepted); they are kept until acknowledged or collected. Cosmetic events are appended by
kernels through `push_event` into a per-tick buffer of `events_per_tick` (64) records whose header
counts every attempt; the decoder rejects a header whose drops do not account for its overflow,
and the ledger keeps the newest events in a bounded ring, counting drops on the device and in the
ring separately. Every tick readback is identity-checked: each result must echo its command's
identity, position and tick. The summary reports the latest observed tick, input latency and
observation age in ticks, tick latency and pass time. In 02 the only cosmetic producer is the
pressure-injection kernel; contacts and immersion (04) are the first real ones.

*Implemented (04).* A `TickBatch` names up to `body_samples_per_tick` (64) bodies to sample;
`body_observe` writes each one's `BodySample` (pose, velocity, ground normal, grounded and
suspended, submerged fraction and liquid, touching count and counters) at the end of the tick,
echoing the handle, or reports it `BodyGone`; the runtime's `BodyMonitor` chooses them (the
controlled body every tick, the others in turn). Important body outcomes are the device's
per-body counters (landings, liquid entries, body contacts, craters, splashes, absorbed energy),
which only grow and wrap in 32 bits: the monitor delivers the difference against a per-body
watermark, so a delayed, reordered, duplicated or skipped sample neither repeats nor drops one,
and spawn and removal outcomes (with the final counters) arrive through command receipts. The
monitor's event ring never overwrites: an event that does not fit is deferred (the watermark
stays) and the next drain delivers it; removals that do not fit wait, one per slot, and only
beyond that are counted unreported, their receipts still holding them. The cosmetic producer is
one `EVENT_BODY` per body per tick, coalescing its craters, splashes, contacts and landings and
its strongest impact; overflow is counted on the device and in the ledger. The ledger adds every
`BodiesCompleted` report into `LedgerSummary::bodies`, and since 07 every tick's cell report into
`LedgerSummary::cells`, so the tools and reactions of a span can be checked against totals
measured at its ends (`resident-world` step 6 does, exactly). Sixty-four samples are 6 KiB per
tick.

The rings are sized from the budgets: 256 commands and their results are 24 KiB per tick, at most
96 KiB per presented frame at the default catch-up of four ticks, inside the 256 KiB budget; the
measured steady payload is in the [performance budgets](performance-budgets.md#measured-transfer-lifecycle-02).
Since 03 every tick also reads back its 144-byte diagnostics record (and 96 bytes per totals
command) and uploads one 4-byte word for chunk wake requests: about 0.6 KiB per frame more.

## Snapshots and completed ticks

A snapshot identifies a completed logical boundary. Reserve independent snapshot storage, order
capture after the requested physical work and before source reuse, then stream out bounded pieces.
The runtime owns when enough payloads are durable to publish a checkpoint; the GPU owner only
reports capture/readback completion. Details are in the world-format contract.

*Implemented (05).* Every slot has a change word (`plan::ActivityLayout::changes`, addressed
through `TickParams::changes_base`). Every kernel that writes a cell value sets its slot's word
(`mark_slot_changed`): the brush edit, the material commit (through `react`, which knows the
tile's changed cells) and the body passes' `commit_cell` (deformation and displacement). An
activation clears the word with its copies at the start of the tick, a snapshot copies it and
clears it at the end of the submission that takes it, and a resize carries it with the kept
chunk. So "unchanged" means unchanged since the slot was activated or last snapshotted, and a
retirement does not clear it: the eviction snapshot that is copied after the retirement still
reads it.

`PhysicalWorld::snapshot_chunk` (an outgoing chunk) copies the slot into an entry of a fixed chunk
image (`TransferConfig::snapshot_entries`, 32) and its change word onto a flag sheet at the end of
the next submission. `snapshot_world` copies the whole cell array, every change word and every
body slot's state and configuration into a world image it allocates for itself, within the plan's
reserved `world_image_bytes` (`8N + 144 B`), and records the table's slots and the tick's physical
configuration. At most one world snapshot is in flight. Each submission that copies snapshots
reserves one capture staging buffer as its sheet (the world's words, then one per chunk snapshot,
at most 17 KiB). When the sheet maps, an unchanged chunk is reported (`Outcome::Snapshot {
changed: false }`) and freed, with no payload; a changed one streams through capture staging in a
later submission and is reported ready; a world snapshot's bodies stream in pieces of at most 32
KiB and are decoded into `world_model::resume::BodySet` (`Outcome::BodySnapshot`, with the number
of chunks it holds and its configuration). A spawn of the copied tick whose target missed is
recorded as the release its outcome will make. The world image is freed when its last chunk and
its bodies are taken. A snapshot is an obligation: lost or faulted, it is abandoned; stopped, a
ready one stays takeable and one that still needed a submission is abandoned.

Copies ride at the end of whichever submission comes next: a tick (the snapshot holds the end of
that tick) or `submit_transfers` (no tick: it holds the end of the last submitted tick, or the
opened checkpoint's before the first). `restore_bodies`, before a session's first submission,
writes a checkpoint's body slots into the table and every live body's records through the chunk
upload staging, copied first in that submission. A resize waits while a snapshot is requested
(its slot indices would move) and proceeds while one drains (the images are independent); the
budget is held against the old allocations, the new window and the larger of the two windows'
world images together (`ResourcePlan::overlap_with`): a world snapshot may still drain the old
window, or be requested for the new one before the old buffers are released, and the
`snapshot-resize-overlap` case measures that peak at exactly this bound. `preflight_resize` checks a resize against the limits and
the budget without allocating, so the residency owner learns of a refusal before it evicts.

## Minimum contract tests

Execute on real GPU resources: packed-word roundtrip; empty and partial workgroups; repeat/reset;
stale command/body/slot rejection; ordered edit plus query; delayed map completion; zero/full
observation queues; repeated resize; controlled device-loss injection; and resource-limit refusal,
with family-specific physics and cross-family tests. Shader validation or CPU emulation alone
cannot satisfy this boundary.

`cargo evidence test gpu --suite transfers --adapter ID` executes these on the named adapter, one
process per case; [`coverage.toml`](../../tools/evidence/coverage.toml) lists the 28 cases
(`target = "gpu:transfers"`) and what each establishes.
`--suite cells` (03) executes the material phases: rule probes against world-model, the ten
baseline simulation intentions, adversarial capacity, ceiling, seam, sleeping-receiver,
reactivation, rebasing and rollover cases, dense/sparse agreement, the unmatched-transfer
specimen and the activity measurement, 31 cases (`target = "gpu:cells"`). `--suite bodies` (04)
executes bodies, rays and observations: the twelve baseline sphere tests and apportionment as
GPU cases, geometry probes, the broadphase oracle, handles and stale generations, the fixed
clock, clusters, fast bodies, shared deformation, edits under a resting body, the residency
edge, rays against an independent reference, ray-targeted commands, controls through receipts,
observation delivery under pressure and the measurements, 32 cases (`target = "gpu:bodies"`).
`--suite streaming` (05, 19 cases) executes residency, snapshots, checkpoints and recovery over
real temporary stores, and `--suite render` (06, 12 cases) the renderer's reads of this state.
`cargo evidence run --scenario resident-world` executes the whole trace through the runtime's
session, the live world and a store, headless or in the native window; its step 6 combines dense
bodies at a seam with edits and phase changes, delayed consumers and readbacks, a rebase across
negative coordinates, failed writes and saves, device loss and process interruption (07).

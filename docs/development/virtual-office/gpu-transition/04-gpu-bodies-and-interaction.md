# 04 — Coupled GPU bodies and useful interaction

Execute after 03. Deliver the existing sphere mechanics against the resident material world, with
useful bounded CPU observations. A renderer-only particle demo or bodies that ignore terrain
changes do not meet this stage. Rendering/window cutover remains 06.

## Read and scope

Read the handoff, acceptance A06/A07, architecture, GPU contract and simulation-correctness practice.
Inspect `spherical.rs`, `grid_physics.rs`, original input/dev tools and all twelve sphere tests plus
the apportionment test. Preserve their intent rather than only the simplest collision case.

## Checkpoint A — Body identity, time and spatial support

Implement generation-bearing body handles, bounded allocation/retirement and GPU-resident position,
velocity and required physical state. CPU ECS components hold intent/configuration and selected
observations, not a second mutable physical pose. Keep body data layouts driven by pass access;
separate frequently read state from infrequent configuration when measurement supports it.

Integrate bodies on the shared fixed physical clock. State exactly which terrain version each
substep sees and when deformation/displacement becomes visible to following bodies/material rules.
Do not reuse the old render-delta integration. Test selected maximum speed, radius, substeps and
travel bound, including a stalled presentation frame. Unsupported parameters are rejected before
admission rather than silently producing tunnelling.

Implement a complete spatial broadphase for the supported shapes/radii/travel. Compare candidates
against a small exhaustive oracle, including boundaries, coincident centres and dense clusters.
A fixed neighbour count or bin capacity is not permission to drop pairs. Preflight/batch exact work
within configured limits; surface a visible admission/pressure outcome when limits cannot support
an operation. Do not change body count during a benchmark to escape clustered costs.

## Checkpoint B — Parallel contact and shared edits

Implement terrain contacts, body-body response, friction/rolling, restitution, grounded control,
gravity, buoyancy, liquid drag, terrain ruts/craters/berms and fluid displacement/splash behavior
within the source-derived feature contract. Preserve wood/metal presets without claiming their
stylized density is a calibrated physical material model.

Separate candidate/contact generation from response. Choose a per-body gather/Jacobi, partitioned
or equivalent bounded solver with explicit iteration/convergence limits. Shared cells and bodies
cannot be written independently by several invocations without a proved ownership/reduction rule.
Two-word cells are not one atomic value.

Collect contested terrain/material contributions and resolve them against actual source availability
and destination capacity before commit. Include multiple bodies and a simultaneous brush targeting
the same cells. Rejected displacement stays represented; saturation must not silently erase fluid
or sand. Make intentional compaction and restitution losses distinct from conservation violations.

Use resource preflight to prevent ordinary supported work from overflowing halfway through mutation.
Where unexpected mid-tick failure is still possible, fault the session without publishing an
unfinished tick as completed or saved; do not claim rollback of in-place edits that was not implemented.

Choose and document the body-at-residency-edge policy now. Bounded pin/request, bounded travel clamp,
or explicit suspension are acceptable if they preserve body identity and state. Missing chunks are
not empty space and wrapped GPU slots are not neighbouring terrain. Implement the runtime/GPU edge
behavior here; 05 completes its persisted suspension/reload lifecycle.

## Checkpoint C — Commands, queries and a real consumer

Route spawn/remove, controlled movement/jump, attract/lift and GPU-resolved brush/query operations
through the existing command owner. Resolve rays against the same column/body geometry used by the
simulation, with bounded traversal and explicit miss/outside-residency outcomes. Do not read the
terrain to the CPU to calculate each edit. Test edit acknowledgement versus eventual observation,
retries, invalid handles and capacity rejection.

Emit selected body pose/grounding/contact/immersion summaries and command outcomes through 02's
bounded channel. Add a real CPU consumer in the runtime/evidence scenario that reacts to these
outcomes, records their tick/age and exposes pressure. A declared event enum alone is not delivery.
Do not build unrelated damage/audio gameplay solely to create an event demo.

Exercise important receipt retention/acknowledgement and cosmetic coalescing separately. Delayed
consumption, reused body slots and saturated queues must not double-apply results or erase important
outcomes. A pressure signal must itself remain observable. Retain no unbounded per-cell changed log.

## Checkpoint D — Physical and latency evidence

Register and execute the `bodies` GPU suite. Preserve all twelve body tests and apportionment coverage
with independent expectations/tolerances. Add clustered and mixed-radius candidates, coincident
centres, high supported speed, body retirement/reuse, simultaneous shared deformation, terrain edits
under a resting body and the declared unloaded-edge policy.

Demonstrate fixed-tick invariance across presentation rates within declared floating tolerances.
Check penetration, resting drift, slope progression, grounding/jump, restitution/energy bounds,
flotation and exact material accounting where promised. Compare visible debug geometry to physical
state when available; do not hide a bad contact by drawing a different pose.

Extend `resident-world` through sand, water, a real GPU-resolved edit and a CPU-consumed observation.
Measure broadphase/contact scaling, solver iterations, coupled GPU time, CPU work, bytes, queue
pressure and observed tick age. Cover a dense cluster, not only separated bodies. Preserve old
all-pairs baseline evidence so the improvement is attributable.

## Acceptance and stop

A06/A07 are complete: bodies and material edits share GPU authority and a fixed clock; collision
candidates and conserved quantities are not truncated; actual interactive commands and CPU consumers
work without a terrain mirror or synchronous per-frame readback. Update limits, solver/order decisions,
body boundary policy and evidence. Commit coherent checkpoints and stop before 05.

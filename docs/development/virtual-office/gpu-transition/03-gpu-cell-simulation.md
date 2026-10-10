# 03 — GPU-resident cell simulation and sparse work

Execute after 02's actual GPU boundary is qualified. Deliver all existing material families on the
GPU, using the packed world as live authority. Do not introduce sphere physics or disk persistence
in this stage, and do not reduce the milestone to liquid alone.

## Read and scope

Read the handoff, acceptance A04/A05, GPU contract, simulation-correctness and GPU/performance
practices. Inspect the original granular, liquid, thermal, fire and biology modules and all related
cell/simulation tests. Use the extracted semantic definitions, resource owner and transfer path.
The legacy app remains the comparison baseline, not the new runtime's fallback.

## Checkpoint A — Explicit physical phases

Write the concrete pass/resource table at its canonical owner before translating rules. Identify
immutable inputs, output versions, shared writes, workgroup/global boundaries and maximum scratch
consumption. Specify the order of cell edits, granular compaction/flow, liquid flow/erosion,
thermal/phase reactions, growth/fire and wake propagation. Preserve intentional old-state versus
post-update reads or record a tested correction; do not accidentally fuse a global dependency into
a workgroup barrier.

Use one fixed logical clock, including configured slower rule cadences and bounded catch-up. Encode
large tick/coordinate values without assuming native shader u64. Define deterministic tie-breaking
and keyed randomness from world identity/seed, position, tick and rule salt. Record the legacy
thread-RNG replacement as a semantic change, not byte-identical behavior preservation.

For each material operation, name its conserved column quantities, intentional sinks/sources and
capacity policy. Correct the elevation-ceiling quench case at its owner with an independent fixture.
Partial conversion must not recurse or retry indefinitely when no capacity remains. Do not invent a per-species mass law the one-label column cannot express. Keep current packed
semantics unless a separately justified conversion and test require a change.

## Checkpoint B — Complete dense GPU baseline

Implement a correct dense path first. Calculate transfer proposals once, resolve accepted receivers
against their capacity, and commit the same accepted amounts at both ends. A different design is
acceptable with a written ownership/conservation argument and discriminating measurement. Rejected
material remains at its source; stale proposal/counter contents cannot leak into a new tick.

Implement water, oil, magma, frozen columns, granular movement/erosion/compaction, thermal phase
changes, combustion and existing biological growth. Preserve configuration toggles and ambient
climate effects. Keep intentional boiling/fuel/tool sources and sinks explicitly accounted for.
Commands reuse the normal admitted edit path; test capacity and frozen/mixed receiving columns.

Use the production assembled shaders and real GPU buffers. Add bounded diagnostic reductions rather
than downloading the whole resident world during ordinary operation. Test total accounting above a
single 32-bit global sum's range without relying on overflow. Full snapshots of small fixtures are
permitted in tests and must be labelled as diagnostic transfers.

## Checkpoint C — Correct sparse scheduling

Replace full-cell idle traversal/copy with measured active-tile work. Derive each tile's dependency
halo from its actual reads, including rules that consult a neighbour's neighbour. An inactive
receiver may still need updating. Edits, climate/configuration changes, chunk activation and relevant
neighbour changes must reactivate sufficient work.

Keep active counts and indirect dispatch arguments on the GPU. A compact metadata scan is allowed;
a CPU count readback every tick or a hidden full-cell pass is not. Handle empty/partial lists,
duplicates, reset, maximum active tiles and supported indirect-dispatch limits. Budget all activity
metadata and scratch in the resource plan.

Unchanged cells must remain valid without the old full-grid copy. Use affected-tile scratch/commit,
versioned storage or another justified scheme. A partial ping-pong swap that reveals stale sleeping
cells is incorrect. Treat toroidal addressing separately from the closed boundary around genuinely
missing/loading world keys. Test negative world keys and translated/rebased fixtures.

Compare dense and sparse execution under identical selected semantics before tuning. Keep a dense
mode only when it is a useful maintained execution/test strategy, not an unbounded duplicate path
that silently replaces broken sparse scheduling. Do not force sparse execution to win on dense work.

## Checkpoint D — Evidence and integration

Register and execute the `cells` GPU suite. Preserve the ten existing simulation and five cell-test
intentions, including fixed-clock behavior; establish new growth/RNG and extreme-capacity coverage.
Add asymmetric donor conflicts, full/frozen receivers, quench at maximum elevation, seam flow,
sleeping receivers, zero active tiles, global climate reactivation and long settlement.

Prove the suite rejects a deliberately unmatched transfer specimen. Compare analytical expectations,
small independent CPU references and dense/sparse outcomes. Do not use candidate-generated snapshots
as the only oracle. Run equivalent tick inputs at different presentation rates and injected long
frames; define and report catch-up time dropped under overload.

Measure dense, sparse, idle and mixed active fractions on the selected adapter. Report physical
passes, scratch bytes, copy/readback traffic and CPU submission overhead. Rework measured hot spots
within scope; do not prematurely expand cells or add speculative workgroup tuning. The current
headless result is not a native-client speedup claim.

## Acceptance and stop

A04/A05 are complete: all material families and controls work through production GPU execution,
material accounting is valid, no normal full-grid CPU mirror is needed, and sparse inactivity does
not corrupt data or retain hidden full-cell work. Relevant original coverage and new adversarial
cases run on the GPU. Update the pass table, rule differences, resource formula, status and handoff.
Commit coherent checkpoints and stop before 04.

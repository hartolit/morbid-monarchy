# Performance practice

Performance work begins with a concrete cost and a falsifiable hypothesis. “Move it to the GPU,”
“use SoA,” and “remove a layer” are design proposals, not evidence of improvement. The accepted
workload, budgets and comparison policy live in [performance budgets](../../reference/performance-budgets.md).

## Describe the cost before changing it

For a changed operation, identify how often it runs, how many items it touches, the bytes read and
written, allocations, synchronization and its limiting resource. Trace the operation through its
consumers. A fast kernel that requires a full readback and upload may worsen the application.

Keep a compact cost account for each substantial phase: cell count/active count; bytes per cell or
body; neighbour probes; scratch writes; dispatch count; and capacity-dependent worst case. This is
an analytical estimate, clearly separate from measured hardware traffic. A profiler counter may
include cache or driver effects that the source estimate does not.

Track three distinct memory costs: CPU retained payload/allocations, GPU requested allocations, and
actual process/device memory where observable. Include steady and peak values. Count upload,
readback, snapshot, frame-in-flight, resize replacement and renderer resources. Summing visible
cell buffers alone is not a memory report.

## Measure the real operation

Record source revision and dirty state; lockfile/toolchain; adapter/backend/driver; device limits
and enabled features; build profile; scenario/configuration/seed hashes; resolution and rendering
quality; frame pacing; warm-up; sample duration; and concurrent workloads. Save raw samples and
machine-readable summaries. Do not stop the user's model servers or other processes without
permission; report contention and obtain a clean run when it is necessary for qualification.

Use release builds. Measure shader/pipeline compilation and initial streaming separately from
steady state, but do not omit them from startup results. Warm the actual workload, then use several
independent runs and report distributions. Alternate baseline/candidate runs where thermal or clock
drift can bias ordering. Establish ordinary variance before interpreting a small improvement.

Measure CPU frame work, GPU cell/body/render phases where timestamps are supported, end-to-end frame
time, input-to-applied-tick latency, observation age, transfer bytes and memory high-water marks.
CPU time around `submit` is not GPU execution time. An asynchronously queued benchmark must account
for completion and not just enqueue work until the run ends. Keep in-flight work bounded.

GPU timestamp samples need feature checks, tick-to-time conversion and asynchronous readback. They
may be unavailable or too coarse for a tiny phase; report that limitation instead of inventing a
number. Separate pass timestamps are diagnostic; do not sum overlapping intervals and call that
frame time. See upstream API references in [sources](../../reference/upstream-sources.md).

## Keep comparisons fair

Run the same semantic workload with the same active world, bodies, commands, tick count, camera,
visible geometry, effects and resolution. Record any intentional behavioral/quality difference.
Compare a shader-only optimization with the same renderer as well as the complete application.
The native renderer may establish a new absolute qualification result, but a cheaper renderer is
not evidence that physics itself became faster.

Do not tune a test's input, lower a body count, increase sleep, shorten simulation, suppress events,
increase observation delay or disable saving merely to improve its timing. A profile reporting a
requested radius that was internally reduced is invalid.

Use correctness gates before accepting timing. Retain useful slow/reference cases and adversarial
inputs. If an optimization changes numerics, define the permissible difference and test its physical
consequences before collecting the final benchmark. Do not choose a tolerance after seeing the error.

## Optimize the largest proven cost

Prefer eliminating work and transfers before instruction-level tuning. Possible changes include
resident state, compact active lists, single evaluation of transfer proposals, spatial broadphase,
reused geometry, derived-data invalidation and pooling. Each must retain its failure/capacity rules.

Then investigate memory access locality, coalescing, unpacking cost, data reuse, divergence,
register pressure, workgroup storage and dispatch overhead. Occupancy is a diagnostic, not a score
to maximize: a higher value can still lose to more memory traffic or worse locality. GPUOpen's
hardware-specific discussion is a source of hypotheses, not a universal tuning prescription.

Do not fuse passes across a required global dependency. Do not split every arithmetic step into a
dispatch either. Compare the reduced traffic of fusion against extra registers, divergence and
synchronization constraints. Use a portable correct path before an optional subgroup/vendor path;
retain both only when they share a contract and their distinct supported purpose is measured.

## Bound hot work and pressure

After warm-up, application-owned per-frame/per-tick pools should reuse capacity. Record allocation
counts around the owned path, separately from opaque driver allocations. A preallocated buffer can
be large and still violate the memory budget; “zero allocations” is not the entire performance goal.

Every queue, contact list, dirty list, staging pool, snapshot backlog and worker set has a bound and
an exhaustion outcome. Pressure must not create unbounded retries, scans or frame stalls. A crowded
body bin can still approach all-pairs work: test clustered distributions, not only evenly scattered
bodies. A ray or brush has a maximum traversal/area and a clear refusal result.

Idle work matters. A sleeping world should not scan/copy every cell just to learn that nothing
changed. Scanning compact tile metadata may be acceptable when measured and budgeted. Reactivation,
climate changes, chunk ingress and edge propagation must remain correct.

## Decide and leave evidence

Accept an optimization only when correctness is preserved and its end-to-end/resource benefit is
shown at a relevant scale. Keep the simpler correct version when results are inconclusive. Record
what was compared, the observed difference and the rejected tradeoff in the owner or evidence
summary—not a permanent journal of every attempted tweak.

A missed gate remains a failure or an explicitly unqualified result. Budgets may change through a
recorded product/architecture decision, never by rewriting the baseline after optimization. Major
algorithm/layout changes require fresh full-profile measurements; small local changes need the
focused cases capable of exposing their regression.

# Performance budgets

**Proposed qualification contract, not benchmark results.** 01 measures the baseline and freezes
profile inputs and concrete resource capacities before optimizing. Values below establish the
initial engineering target; they are not prior promises from the user or measured device limits.
A material change needs a recorded rationale before candidate tuning, not a post-failure rewrite.

## Profiles

The initial hardware nomination is the user's Linux desktop with Ryzen 9 5950X and RTX 5070 Ti.
The runner must discover and record the actual adapter, driver, memory and settings. A model server
or another heavy workload makes a run contended; it must be identified. No hardware lane is assumed
available to an agent merely because the user owns it.

| Profile | World / bodies | Purpose and gate |
| --- | --- | --- |
| `micro` | Tiny hand-authored grids; up to a few bodies | Exact contracts and fault sequences; no performance claim. |
| `qualification` | Radius 6 on both axes, 64×64-cell chunks, 64 bodies, 1920×1080, 60 physical ticks/s | Initial interactive target: frame p95 ≤16.67 ms and p99 ≤25 ms, bounded work, all required behavior/effects declared. |
| `scale` | Radius 16, 256 bodies, same tick rate and presentation settings | Measure growth, isolate CPU/GPU/transfer limits; no unmeasured 60 FPS guarantee. Correctness/resource bounds remain mandatory. |
| `capacity` | Radius up to 32, up to 4,096 bodies in distributed and clustered cases | Verify admission, limit diagnostics, complete interactions or explicit refusal before unsafe work. Capture memory peak and worst-case work; no fixed FPS promise. |

For each profile 01 writes immutable scenario data: seed, geometry, material distribution, camera
path, ordered commands, body properties, simulation flags, quality settings and sample durations.
Use separate `idle`, `flow`, `body-contact`, `mixed`, `streaming` and `coupled` cases. A dense-active
case is required; never benchmark only a world that already fell asleep. The qualification case
includes meaningful terrain, water, granular movement and body interaction. Fire/growth/phase rules
also run in their defined mixed case rather than being silently excluded from the product.

## Baseline arithmetic

For chunk edge C=64 and radii rx,ry:

`N = (2*rx+1) * (2*ry+1) * C*C` cells; one packed cell copy is `8*N` bytes.

| Equal radius | Cells | One cell array | Array payload ×60/s |
| --- | ---: | ---: | ---: |
| 6 | 692,224 | 5.28125 MiB | 0.30945 GiB/s |
| 16 | 4,460,544 | 34.03125 MiB | 1.99402 GiB/s |
| 32 | 17,305,600 | 132.03125 MiB | 7.73621 GiB/s |

These are calculated payload sizes, not measured PCIe bandwidth. Actual traffic includes or avoids other transfers according to the implementation and hardware. On UMA, the same API copies can consume shared-memory
bandwidth rather than a discrete PCIe link.

## Structural gates

These gates are independent of an arbitrary FPS target.

| Resource | Required result |
| --- | --- |
| CPU live physical state | No full resident terrain mirror, CPU body authority, or retained active-chunk cell payload in normal operation after cutover. |
| Steady physical transfers | No per-frame/tick full-grid upload/readback. Commands and selected observations are counted separately from streaming/checkpoints. |
| GPU/CPU synchronization | Zero blocking completion waits on the normal frame path. Nonblocking polling is allowed; explicit maintenance is labelled. |
| In-flight submissions | Bounded by a configured admission limit; initial target at most 3 outstanding frame/tick batches. No unbounded enqueue-only benchmark. |
| CPU hot allocations | Reuse application-owned recurring pools after warm-up; report counts and justify residual allocations. Driver allocations are reported separately when observable. |
| Idle simulation | No recurring full-cell-array copy or traversal for an unchanged sleeping world; compact metadata processing is measured. |
| Pressure | No silent contact/material/event loss, workload shrink or abandoned dirty snapshots. Backpressure and refusal remain visible. |

### Initial memory/transfer envelopes

Use these starting envelopes for allocation planning, then validate them in 01–02 before freezing
their effective values. If they cannot support required behavior, show the complete accounting and
choose an alternative before kernel tuning. Do not fit the final report by omitting resources.

- GPU terrain-related state, including cell scratch, activity/proposals and a bounded checkpoint
  image: target at most `48*N` bytes plus explicit per-chunk/fixed overhead. This is an envelope,
  not a requirement to allocate 48 bytes for every cell. Body/renderer resources have separate
  declared formulas and are included in the total peak.
- Combined CPU upload/readback/save-retry payload working set: initial budget 64 MiB. Large
  checkpoints drain in bounded pieces from an independent snapshot; the CPU need not hold them
  all at once. Metadata and any optional clean cache have their own byte-limited accounts.
- Command plus ordinary observation payload: initial qualification target ≤256 KiB per presented
  frame and ≤16 MiB/s at the target cadence, excluding labelled load/save traffic. Count actual
  bytes; an empty event stream is not proof that interaction works.
- Selected controlled-body observation age: p95 no more than two physical ticks under the
  qualification workload. Record input-to-applied-tick and presentation lag separately.

Allocation admission must account for old and replacement buffers coexisting during resize,
checkpoint staging, render targets, textures, body pools and delayed callbacks. Device limits are
hard constraints, not these engineering targets. Binding size is separate from allocation size;
request a supported larger binding or use an explicitly qualified layout/refusal policy.

## Regression and improvement policy

Measure at least five independent warmed runs of a profile; a starting protocol is 300 warm-up
frames/ticks followed by at least 1,800 measured frames, with raw samples retained. Extend a run
when variation obscures the conclusion. Record zero-simulation frames and multiple-tick frames.
Do not exclude long frames merely as outliers without a documented external cause.

The target is no unexplained >5% regression in a required scenario's median or p95 outside the
measured noise range. This is a review threshold, not permission to regress 4.9% in every stage.
The final qualification must also show reduced simulation-related CPU work and eliminated steady
full-grid transfer versus the comparable baseline. Report ratios rather than promising an
unmeasured 2× or 10× improvement.

If renderer quality changed, keep a same-renderer/simulation-only comparison and a separate full
native-client qualification. A budget miss is reported with the responsible phase and evidence.
Do not silently lower the resolution/body count or move the target after measuring.

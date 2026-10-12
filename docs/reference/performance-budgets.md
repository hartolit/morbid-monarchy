# Performance budgets

**Frozen qualification contract plus one measured baseline.** 00 adopted the evidence policy in
[ADR 0003](../decisions/0003-behavior-and-performance-evidence.md). 01 froze the profile inputs as
scenario files, calibrated the run protocol, and measured the legacy application
([below](#measured-legacy-baseline-01)). The targets are engineering targets, not prior promises
from the user or measured device limits. A material change needs a recorded rationale before
candidate tuning, not a post-failure rewrite.

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

## Measured legacy baseline (01)

Executed evidence, measured on 2026-10-12 between 17:03 and 18:16 UTC with
`cargo evidence bench --profile qualification|scale --adapter vulkan-10de-2c05`.

| Identity | Value |
| --- | --- |
| Source | `6e108d6`, clean tree; Rust 1.99.0; `Cargo.lock` SHA-256 `b2cfafe6…`; release build |
| Application | The Bevy baseline (`morbid-app`) in scripted input mode, driven by `legacy-baseline` |
| Host | Ryzen 9 5950X (32 threads), 31 GiB, Linux 7.2.8, KDE Wayland |
| Adapter | RTX 5070 Ti, Vulkan, NVIDIA 615.71.09; kwin, Discord and zed-editor also held GPU memory |
| Presentation | 1920×1080 window, always on top, `no-vsync`, updated continuously focused or not; the application's full renderer and developer overlay |
| Protocol | Five valid runs per case, plus one diagnostic run with GPU timestamps; 300 warm-up ticks, 1,800 measured ticks |
| Validity | No run contended. Three attempts were rejected because the compositor resized the window mid-run (qualification body-contact and mixed, scale mixed) and were repeated; all attempts are in the reports |
| Records | Summaries: [`tools/evidence/baselines/legacy-6e108d6/`](../../tools/evidence/baselines/legacy-6e108d6/). Raw samples and logs exist only in `target/evidence/baseline-{qualification,scale}/` on the nominated machine, listed by SHA-256 in the summaries; `cargo clean` deletes them |

Frame times are in milliseconds; p95 is the median of the run p95s, and "worst" is the highest
run's value. Simulation and body time are per tick. Transfers are per second of measured wall
time. Retained payload is the packed cells the CPU keeps: both grid buffers, wake arrays, active,
cached and queued chunk payloads, and the render asset's copy.

**Qualification** (radius 6, 64 bodies):

| Case | Median | p95 (worst) | Worst p99 | Ticks/s | Simulation / body | Grid upload / full copy | Retained / RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| idle | 3.58 | 6.20 (6.45) | 8.03 | 60.0 | 2.20 / 0 | 0.31 / 0.31 GiB/s | 30.9 / 810 MiB |
| flow | 11.04 | 14.56 (14.76) | 16.79 | 60.0 | 8.98 / 0 | 0.54 / 0.31 GiB/s | 30.9 / 798 MiB |
| body-contact | 14.68 | 18.68 (19.21) | 25.95 | 60.0 | 6.84 / 5.32 | 0.37 / 0.31 GiB/s | 30.9 / 795 MiB |
| mixed | 4.59 | 12.22 (12.48) | 14.17 | 60.0 | 6.81 / 0 | 0.38 / 0.31 GiB/s | 30.9 / 819 MiB |
| streaming | 3.57 | 6.07 (12.28) | 13.56 | 60.0 | 2.10 / 0 | 0.31 / 0.31 GiB/s | 60.4 / 889 MiB |
| coupled | 17.12 | 28.64 (28.83) | 32.10 | 60.0 | 8.32 / 4.81 | 0.29 / 0.31 GiB/s | 30.9 / 790 MiB |

**Scale** (radius 16, 256 bodies):

| Case | Median | p95 (worst) | Worst p99 | Ticks/s | Simulation / body | Grid upload / full copy | Retained / RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| idle | 28.22 | 36.67 (43.44) | 51.75 | 60.0 | 7.64 / 0 | 1.16 / 2.00 GiB/s | 163.1 / 845 MiB |
| flow | 147.86 | 160.39 (161.92) | 170.87 | 29.1–29.8 | 30.31 / 0 | 0.25 / 0.98 GiB/s | 163.1 / 829 MiB |
| body-contact | 205.91 | 218.14 (222.05) | 231.64 | 19.8–20.1 | 29.55 / 16.99 | 0.17 / 0.66 GiB/s | 163.1 / 836 MiB |
| mixed | 170.33 | 206.86 (242.67) | 299.38 | 22.0–23.4 | 39.64 / 0 | 0.19 / 0.76 GiB/s | 163.1 / 845 MiB |
| streaming | 27.03 | 52.63 (70.40) | 101.50 | 57.6–60.0 | 8.71 / 0 | 1.08 / 1.98 GiB/s | 182.4 / 1052 MiB |
| coupled | 223.80 | 252.64 (384.81) | 637.37 | 16.6–18.2 | 34.57 / 17.03 | 0.15 / 0.61 GiB/s | 163.1 / 833 MiB |

What the baseline establishes:

- **It does not meet the qualification target.** Body-contact (p95 18.7 ms) and coupled
  (p95 28.6 ms, p99 32.1 ms) miss p95 ≤16.67 ms and p99 ≤25 ms. Idle, flow, mixed and streaming
  meet them. The baseline is the comparison point, not a passing candidate.
- **Idle work is not idle.** With zero awake cells the simulation still copies the whole cell
  array every tick (0.31 GiB/s at radius 6, 2.00 GiB/s at radius 16, matching the arithmetic
  above), and the renderer uploads it whenever it changed. At radius 16 an idle tick costs
  7.6 ms of CPU.
- **The scale profile falls behind real time.** In every active case the catch-up cap is reached
  on every frame (all 450 frames hold four ticks), the simulation completes 17–30 ticks/s, and
  30–69 s of simulated time are discarded per run.
- **Body contact grows quadratically.** The baseline tests all n(n−1)/2 pairs every substep:
  at most 12,096 tests in a frame with 64 bodies and 261,120 with 256.
- **GPU render passes are small at qualification size.** Diagnostic medians: main opaque
  1.75–1.77 ms, transparent 0.40–0.46 ms, everything else under 0.1 ms; at scale 10.7–11.0 ms
  and 2.9 ms. GPU memory that nvidia-smi attributed to the process peaked at 510 MiB (574 MiB at
  scale).
- **Streaming loads are scheduling-dependent but complete.** Each qualification run loaded 1,079
  chunks (8 from the store), evicted 464 and wrote 456 (14.3 MiB encoded) without a write
  failure. At scale the split between generated and stored loads varied between runs
  (2,097–2,121 generated of 2,934).
- **Variation.** Run medians vary by 2–10% between runs in most cases. One qualification
  streaming run (p95 12.3 ms against 6.0–6.3 ms) and the scale idle, streaming and coupled tails
  varied more; GPU utilization from other processes is recorded around each run but is not a
  quiet-host condition. Use the per-run values in the summaries, not one run, as the noise range.

**After the model extraction (01).** The same protocol at `7a84a80`, after `world-model` replaced
the engine's cell, material and generation code, for flow, body-contact and coupled
(`--case flow --case body-contact --case coupled`; 5/5 valid runs each, none contended; report in
`target/evidence/drift-7a84a80/` on the measuring machine). Simulation time per tick moved by −1% to
−2% (flow 8.90 against 8.98 ms) and body time by 0% to +1% (body-contact 5.37 against 5.32 ms),
within the run-to-run spread above. Frame medians were 3–6% lower (flow 10.34 against 11.04 ms,
body-contact 14.15 against 14.68, coupled 16.68 against 17.12) and p95 0–5% lower; the change is
outside the simulation and body phases and its cause was not isolated. No regression.

The capacity scenarios are frozen but were not run against the baseline: it has no admission or
refusal path, tests all body pairs, and keeps a radius-32 CPU grid of 132 MiB per array. 04–07
run capacity against the GPU runtime.

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

Use these starting envelopes for allocation planning. 01 kept them unchanged: the baseline has
no GPU allocations to check them against, and its CPU payload (30.9 MiB at radius 6, 163.1 MiB at
radius 16) is the arrangement they replace. 02 validates them against actual allocations before
freezing their effective values. If they cannot support required behavior, show the complete accounting and
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

Measure at least five independent warmed runs of a profile, with raw samples retained: 300
warm-up ticks, then a window of 1,800 measured ticks (30 s of simulated time at 60 ticks/s).
Extend a run when variation obscures the conclusion. Record zero-simulation frames and
multiple-tick frames. Do not exclude long frames merely as outliers without a documented external
cause.

*Calibration recorded in 01, before any candidate.* The starting protocol counted 1,800 measured
frames. A frame window measures a different amount of simulated work in every run and every
candidate, because the catch-up cap (4 ticks per frame) decides how many ticks a frame holds: at
the scale profile the baseline presents only 450 frames for 1,800 ticks. A fixed tick window
gives every run the same simulated workload; a scenario's actions are scheduled by tick for the
same reason. The run reports its frame count and flags fewer than 1,800 frames, but still counts.
A run validates only when the measured frames span exactly the tick window, the window keeps its
requested size, the frame times show no compositor pacing (30 or more consecutive frames within
0.15 ms of a 60/120/144/165/240 Hz interval, or at least 1% of frames on one interval and more
than four times as many as 0.45–0.75 ms beside it), and the
host was quiet (all-core CPU busy below 15% before the run, waiting up to 120 s; otherwise the run
is marked contended). A presentation rejection is retried at most twice per case and every
attempt stays in the report; a crash, timeout or simulation error is never retried.

The target is no unexplained >5% regression in a required scenario's median or p95 outside the
measured noise range. This is a review threshold, not permission to regress 4.9% in every stage.
The final qualification must also show reduced simulation-related CPU work and eliminated steady
full-grid transfer versus the comparable baseline. Report ratios rather than promising an
unmeasured 2× or 10× improvement.

If renderer quality changed, keep a same-renderer/simulation-only comparison and a separate full
native-client qualification. A budget miss is reported with the responsible phase and evidence.
Do not silently lower the resolution/body count or move the target after measuring.

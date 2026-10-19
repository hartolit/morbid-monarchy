# Performance budgets

**Frozen qualification contract, the measured baseline (01), the measured transfer lifecycle
(02) and the material phases' resources and measurement (03).** 00 adopted the evidence policy in
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

## Measured transfer lifecycle (02)

Executed evidence of the GPU boundary, not an application speedup: no physical rule runs on the
GPU before 03, so nothing here compares with the baseline's frame times. Measured on 2026-10-13
08:52 UTC at `f740577` (clean tree) with `cargo evidence test gpu --suite transfers --adapter
vulkan-10de-2c05 --output target/evidence/final-02-transfers` (RTX 5070 Ti, Vulkan, NVIDIA
615.71.09; release build), case `steady-state-reuse`: a radius-2 window with nine generated
chunks, 120 warm-up ticks, then 1,200 measured ticks of 8 brush strokes (radius 3) and 8 column
queries each, a capture every 60 ticks and a chunk retired and uploaded every 120. The harness
submits as fast as backpressure allows, so its tick rate is not a frame rate.

| Measured over 1,200 ticks | Value |
| --- | --- |
| Buffers and pipelines created after warm-up | 0 and 0; requested device bytes constant at 3,125,396 |
| Blocking waits on the submit/poll path | 0 (maintenance waits: 0) |
| Submissions in flight, high water | 3 (the limit) |
| Staging buffers in use, high water | upload 5 of 36, readback 4 of 36 |
| Upload per tick | commands 768 B, tick parameters and event reset 80 B, slot metadata 0.6 B; chunk uploads 273 B (residency churn, labelled streaming) |
| Readback per tick | results 768 B, event buffer 1,040 B, timestamps 16 B; captures 546 B (labelled) |
| Command plus ordinary observation payload | 2,673 B per tick: 10.7 KiB per presented frame at four ticks, 160 KiB/s at 60 ticks/s (budgets: 256 KiB and 16 MiB/s) |
| Command pass device time (timestamps, period 1 ns) | median 24 µs, maximum 179 µs (the maximum ranged 38–182 µs over three runs that day) |
| Submission to decoded results | median 0.09 ms, p95 0.16 ms, maximum 1.9 ms (unpaced) |
| Process heap allocations per tick | 105: 103 on the submit path (wgpu's command recording and tracking, and the boxed completion and map callbacks its API takes), 2.1 on the poll path; application-owned pools allocate nothing after warm-up |

The full-resolution case reports and every case's metrics are in `report.json` and
`gpu-transfers/*.json` of the run directory; a later run regenerates them. Per-tick allocations
inside wgpu are reported, not tuned: the budgets' allocation gate applies to application-owned
pools, which this run shows are reused.

**Planned allocations against the envelopes** (default transfer configuration: 256 commands and
64 events per tick, 32 chunk upload and 32 capture buffers, timestamps off). Terrain-related state
is the cell array (8 B per cell), one window entry and one slot record per chunk (36 B per chunk);
staging is 2,213,824 B (2.1 MiB) at every radius; fixed device buffers are 91,216 B.

| Equal radius | Terrain-related bytes | Envelope `48*N + 64` B per chunk | Device bytes requested |
| --- | ---: | ---: | ---: |
| 6 | 5,543,876 | 33,237,568 | 7,848,916 |
| 16 | 35,723,556 | 214,175,808 | 38,028,596 |
| 32 | 138,596,900 | 830,939,200 | 140,901,940 (measured: `radius-32-allocation`) |

A resize keeps the old window buffers until the migrating submission completes: measured radius 1
to radius 2, 2,600,276 B before, 3,125,140 B after, 3,420,376 B at the peak, which is the old total
plus the new window's buffers, released afterwards. The CPU staging working set (2.1 MiB) is
within the 64 MiB budget; 03 added flow, scratch and activity to the terrain account
([below](#material-phases-03)) and 05 adds the snapshot image, against the same envelope.

## Material phases (03)

**Resources.** The material phases add three window buffers and two fixed ones to the plan
(`gpu_world::plan`): flow words (a proposal and a resolution, 8 B per cell), scratch cells (8 B per
cell), the activity buffer (a 16-word header, one wake-request word per chunk, candidate and
process lists with room for every tile, and a 64-byte record per 8 × 8 tile: 1.125 B per cell),
the dispatch-argument buffer (24 B) and the diagnostics buffer (144 B plus 96 B per command
position, 24,720 B). Terrain-related state is `25.125 N + 40 C + 64` bytes for `N` cells and `C`
chunks, against the `48 N + 64 C` envelope; nothing in it is appended, so it cannot overflow at
full activity. Calculated from the plan (the radius-6 values are also the measured plan of the
run below):

| Equal radius | Terrain-related bytes | Envelope | Device bytes requested (timestamps off) |
| --- | ---: | ---: | ---: |
| 6 | 17,398,952 | 33,237,568 | 19,828,080 |
| 16 | 112,114,792 | 214,175,808 | 114,543,920 |
| 32 | 434,972,264 | 830,939,200 | 437,401,392 |

The largest single buffer and binding are unchanged: flow and scratch are the size of the cell
array, 138,444,800 B at radius 32, the binding the device is already asked for. A resize keeps
the old flow, scratch and activity buffers with the old cells until the migration completes.

**Per-tick traffic.** Each tick uploads 148 B without commands (tick parameters 128, event header
16, one wake-request word 4) plus 48 B per command and 36 + 4 B per slot change, and reads back
1,232 B without commands (events 1,040, diagnostics 144, timestamps 48 when enabled) plus 48 B per
command and 96 B per totals command; device-side clears of the list counts, the chunk wake
requests and the diagnostics are `64 + 4 C + 144` B, plus 4,096 B per activated chunk. The
material passes write 16 B per awake cell (flow words and scratch) and 8 B per changed cell
(commit) in sparse execution, and 24 B per resident cell in dense execution.

**Measured on the nominated hardware.** Run on 2026-10-17 at 17:52 local time after a reboot, at
`66ce775` plus the harness fixes committed with these numbers, with `cargo evidence test gpu
--suite cells --adapter vulkan-10de-2c05` (RTX 5070 Ti, Vulkan, NVIDIA 615.78.08, Linux
7.2.10; release build; timestamps period 1 ns). Case `activity-measurement`: the qualification
window (radius 6, 169 chunks, 10,816 tiles, 692,224 cells) of generated terrain (seed 69), 40
measured ticks per scenario after 10 warm-up ticks, one tick in flight, so the round trip is a
latency, not a throughput. Device times are the three compute passes' timestamps; "encode" is the
CPU time of the `submit_tick` call.

| Scenario | Tiles visited | Awake cells per tick | Activity pass median | Material pass median (p95) | Encode median (p95) | Round trip median |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Sparse, idle (settled, deterministic rules) | 0 | 0 | 9 µs | 18 µs (18) | 47 µs (54) | 0.26 ms |
| Sparse, mixed (8 water strokes a tick) | 1,845 (17%) | 95,878 | 14 µs | 92 µs (130) | 61 µs (73) | 0.26 ms |
| Sparse, growing (every rule, foliage spreading) | 8,619 (80%) | 500,653 | 29 µs | 186 µs (192) | 50 µs (60) | 0.26 ms |
| Sparse, full (a climate change every tick) | 10,816 | 692,224 | 38 µs | 242 µs (250) | 62 µs (81) | 0.51 ms |
| Dense, idle | 10,816 | 0 | 34 µs | 52 µs (52) | 48 µs (78) | 0.26 ms |
| Dense, full | 10,816 | 692,224 | 38 µs | 242 µs (248) | 54 µs (78) | 0.51 ms |

An idle sparse window visits no tile and touches no cell: its 27 µs are the scan of one word per
tile and four empty indirect dispatches, against 86 µs for dense execution of the same world. At
full activity sparse costs what dense costs (242 µs, 0.35 ns per cell), as expected; sparse
execution is not tuned to win on dense work. The command pass with eight radius-6 strokes takes
25 µs. Per tick the upload is 148 B without commands and the readback 1,232 B (1,280 B with one
configuration command); the material passes store 1.9 MB (mixed) to 11.1 MB (full) on the device,
16.6 MB in dense execution, by the count above. Every measured tick balanced its accounting, no
buffer was created and no frame wait occurred. Process allocations were 110–142 per tick, all on
the submit path inside wgpu (command recording for three passes); application pools allocate
nothing after warm-up. At radius 6 four fully active ticks per frame cost about 1.1 ms of device
time against the 16.67 ms frame target, so 03 found no hot spot to rework. These are physics-only
device numbers on the headless owner, not a native-client frame time or a speedup claim against
the baseline.

The same case ran on Mesa llvmpipe (`gl-10005-0000`) while the NVIDIA driver was mismatched, with
the same tile and cell counts; llvmpipe executes compute during the submit call, so its times are
CPU emulation and are not repeated here.

**Transfer path re-measured at 03.** `steady-state-reuse` (the 02 workload above, with every
material rule off but the material passes running) on the same adapter: command pass median 31 µs
(24 µs at 02: edits now also mark wake and count tool quantities), whole tick 74 µs, tick latency
median 0.22 ms and p95 0.39 ms, 163 process allocations per tick (160 on the submit path, 105 at
02), no buffer created, no frame wait, readback +144 B and upload +4 B per tick for the
diagnostics and the wake-request word.

**Catch-up under overload.** The clock discards owed ticks beyond the per-frame cap; the dropped
simulated time is the discarded ticks times the tick duration (`Runtime::discarded_ticks`). In
`presentation-rates` a 400 ms stall at 60 Hz owes 24 ticks, runs 4 and discards 20 (333 ms); the
stalled run discarded exactly 20 per stall (320 ticks, 5.3 s over 16 stalls) and reached the same
world at the same tick as the 30, 60 and 144 Hz runs, which discarded nothing.

## Bodies (04)

**Resources.** Body state is sized by the body capacity `B` (default 4,096) and the samples per
tick `S` (default 64), not by the window: the bodies buffer holds `92 B + 26 S` words (state 32,
configuration 4 and solver work 56 words per body; observe list and sample records), the bins
`3 H + B + 4` words with `H` the hash table, a power of two at least `2 B`, and the dispatch
table one 256-byte entry per (substep, pass), 192 entries. At the defaults that is 1,513,984 B of
bodies, 114,704 B of bins and 49,152 B of dispatch table: 1.6 MiB in all. The window gains one
claim word per cell (`4 N`), so terrain-related state is `29.125 N + 40 C + 64` bytes against the
`48 N + 64 C` envelope: 20,167,848 B at radius 6, 129,956,968 B at radius 16 and 504,194,664 B at
radius 32. Nothing is appended and no list has a capacity a cluster can exceed: bins have no
per-bin capacity, a footprint fits its workgroup's 27 × 27 cells by the radius limit, and the
event and sample buffers are bounded with their overflow counted.

**Per-tick traffic.** On top of 03's, each tick uploads the 128-byte body parameters and 8 B per
sampled body (handle) and reads back the 128-byte body diagnostics, 96 B per sample and, with
timestamps, 64 B for eight stamps: with 64 samples and no command, 788 B up and 7,520 B down;
each command adds 112 B up and 96 B down. At 60 ticks/s the samples are 0.35 MiB/s and the whole
readback 0.43 MiB/s, inside the 16 MiB/s observation budget.

**Measured on the nominated hardware.** Run on 2026-10-18 at 12:48 local time, at `e8ee688`
with a clean tree, with `cargo evidence test gpu --suite bodies --adapter vulkan-10de-2c05`
(RTX 5070 Ti, Vulkan, NVIDIA 615.78.08, Linux 7.2.10, Ryzen 9 5950X; release build;
timestamps period 1 ns). Device times are the body pass's timestamps (every substep, from the bin
clear to the re-anchoring, and the observation); "encode" is the CPU time of
the `submit_tick` call. Each tick is submitted alone and waited for, 64 bodies are sampled a
tick, and the default solver runs 16 position and 16 velocity passes, two substeps a tick at
60 Hz.

*Scaling* (`body-scaling-bins`, `body-scaling-all-pairs`): flat stone, every material rule off,
a radius-3 window, 30 measured ticks after the warm-up. *Distributed*: radius-2.5 spheres six
cells apart, resting. *Dense*: radius 2–5 spheres in up to four layers, attracted to one column
for 120 ticks and measured while still attracted, a pile under load. Pair visits are the
candidate slots the count, position and velocity passes examined; candidates are the pairs that
passed the generation test; contacts are counted once per substep.

| Bodies | Layout | Bins: body pass median (p95) | All pairs: median (p95) | Pair visits per tick, bins / all pairs | Candidates per tick | Contacts per tick |
| ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 64 | distributed | 244 µs (260) | 240 µs (257) | 968 / 8,192 | 448 | 0 |
| 256 | distributed | 259 µs (269) | 261 µs (269) | 5,102 / 131,072 | 1,920 | 0 |
| 1,024 | distributed | 370 µs (380) | 444 µs (455) | 24,492 / 2,097,152 | 7,936 | 0 |
| 4,096 | distributed | 947 µs (963) | 4.18 ms (4.51) | 168,032 / 33,554,432 | 32,256 | 0 |
| 64 | dense | 671 µs (711) | 526 µs (571) | 165,387 / 270,063 | 27,546 | 564 |
| 256 | dense | 1.49 ms (1.67) | 1.40 ms (1.58) | 1,286,819 / 4,307,627 | 140,323 | 2,891 |
| 1,024 | dense | 2.18 ms (2.41) | 5.64 ms (6.26) | 7,883,352 / 69,059,652 | 651,974 | 13,966 |
| 4,096 | dense | 8.21 ms (8.86) | 97.1 ms (98.7) | 73,400,940 / 1,105,858,833 | 4,596,233 | 92,666 |

Both broadphases solve the same pairs, so their candidates, contacts and residuals are equal;
the bins win from about a thousand bodies (12 times at the dense 4,096), and at 64–256 bodies,
where a body's 27 bins already hold most of a small cluster, the all-pairs slot list is about as
cheap. Solver passes at the dense 4,096: 4 + 4 passes take 12.2 ms (p95 13.3), 8 + 8 take
10.1 ms and the default 16 + 16 take 8.2 ms; fewer passes leave the pile more compressed
(786,722, 250,268 and 92,666 contacts a tick, 13.3, 7.6 and 4.6 million candidates), so the
extra passes pay for themselves. Under attraction the deepest residual overlap is 10.0 units at
4,096 bodies (0.17 at 64): the pile is being crushed faster than the 0.5-unit-per-substep
correction bound resolves it; settled, the dense-cluster case measures at most 0.31 of the
smaller radius. Encode takes 190–360 µs (p95 up to 580 µs at the dense 4,096), and the round
trip is 0.52 ms (distributed up to 1,024) to 8.5 ms (bins, dense 4,096).

*The legacy workloads* (`body-legacy-workloads`): the baseline's frozen `body-contact` scenarios,
checked against the SHA-256 its summaries record, replayed tick for tick (300 warm-up and 1,800
measured ticks, seed-69 terrain, every rule family, the scenario's window):

| Profile | Bodies | Baseline body time per tick (CPU) | GPU body pass median (p95, max) | Whole tick on the device | Pair tests per tick, baseline / pair visits | Encode median (p95) | Round trip median |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| qualification (radius 6) | 64 | 5.32 ms | 0.64 ms (0.84, 1.64) | 0.81 ms | 4,032 / 96,122 | 206 µs (384) | 1.03 ms |
| scale (radius 16) | 256 | 16.99 ms | 1.00 ms (1.41, 2.49) | 1.76 ms | 65,280 / 651,519 | 304 µs (435) | 2.03 ms |

The baseline's number is CPU time of its body systems, the GPU's is device time of the body pass;
the CPU work that remains per tick is the encode. The GPU examines more candidate slots than the
baseline tested pairs (its 32 passes each read the candidates again), but in parallel. Each
workload rolled, rutted (23,134 and 66,478 ruts), cratered (3,796 and 7,390) and contested cells
(253,992 and 484,198 lost claims) with every tick balanced. These are physics-only device numbers
on the headless owner, not a native frame time; the frame-time comparison is 07's.

*Before and after the per-body workgroups.* Measured during 04 at uncommitted intermediate
states of the same cases, with one thread per body walking its candidates: the qualification
workload's body pass took 7.6 ms (more than the baseline's 5.32 ms CPU time) and the dense 64-body
pile 9.8 ms, more than 90% of it in the pair passes (0.64 ms with them skipped), which found about
400 candidates a pass: latency-bound serial work, not volume. Sharing the candidates over a
workgroup brought them to 3.0 and 2.9 ms, and finding the 27 bins one lane each to 0.64 and
0.67 ms, with the same candidates and residuals throughout.

*Pacing* (`body-pacing`): the runtime presenting 180 frames of 60 Hz wall time, with up to three
ticks in flight, after the warm-up.

| Workload | Ticks per second | Discarded | Frames under pressure | In flight, high water | Frame CPU median (p95, max) | Tick latency mean | Body sample age, max (mean) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 256 distributed | 60.0 | 0 | 2 | 3 | 0.40 ms (0.68, 1.22) | 16.5 ms | 2 ticks (1.1) |
| 4,096 dense, attracted | 60.0 | 0 | 3 | 3 | 0.37 ms (0.51, 1.12) | 17.0 ms | 2 ticks (1.4) |

No frame waited on the device; the controlled body's sample was one tick old at the end of both
runs, inside the two-tick budget for selected observation age.

**Allocations.** Polls allocate 3 times a tick. The submit path allocates 336 times a tick
(distributed) and 347 (dense, one command): 167 without bodies, as `steady-state-reuse` measures
it, and the rest in proportion to the body pass's dispatches (249 at 4 + 4 passes, 282 at 8 + 8,
347 at 16 + 16, about two per dispatch). The encode path makes only wgpu pass calls between its
reused buffers, so these are wgpu's command recording (an inference from the source, as at 03);
fewer, larger dispatches would reduce them, which 06–07 can measure against the frame budget.

## Streaming and checkpoints (05)

**Resources.** On the device, snapshots are planned beside live terrain: a fixed chunk snapshot
image of 32 entries (1 MiB) and the world image a world snapshot allocates while it drains,
`8 N + 144 B` bytes (6,127,616 B at radius 6 with the default 4,096 bodies), reserved in the plan
so the budget holds while one drains; one change word per slot. During a resize the budget holds
the old allocations, the new ones and the larger world image together
([GPU contract](gpu-contract.md#snapshots-and-completed-ticks)). On the CPU the residency owner
keeps metadata per key, never cells, and two byte budgets: payloads loaded and waiting for upload
(16 MiB) and payloads captured and not yet durable (32 MiB), 48 MiB together against the 64 MiB
working-set envelope below. At most 16 loads are in flight; the workers' queues hold 32 loads and
16 writes. Over its budget the owner stops moving the window and reports the pressure.

**Measured on the nominated hardware.** Run on 2026-10-19 at 12:46 UTC with
`cargo evidence test gpu --suite streaming --adapter vulkan-10de-2c05` on the tree committed as `6f6297c`
(RTX 5070 Ti, Vulkan, NVIDIA 615.78.08, Linux 7.2.11, Ryzen 9 5950X; release build). A frame's CPU
time is the `WorldSession::frame_ticks(1)` call: the runtime's frame (polling, encoding and
submitting the tick) and the residency and checkpoint owners. Frames are paced 0.5 ms apart so the
device keeps up; there is no presentation, so this is not a native frame time. Allocations are
the process's, counted by the runner's allocator, including the store's worker threads and wgpu.
The window is radius 6 (169 chunks) of generated terrain, seed 69, with every rule on.

*Ordinary resident simulation* (`resident-no-readback`): 600 ticks with water poured at
the centre, no move and no save, uploaded no chunk, copied and read back no snapshot or capture and
submitted no transfer. Each tick read back 1,312 B (results, events,
accounting, samples) and uploaded 276 B.

*A save of the resident window* (`streaming-measure`): the world snapshot copied all 169 chunks on
the device (5,537,792 B) and read back only the
112 the flowing water had changed (3,670,692 B: their payloads
and the flag sheet), in 5 transfer submissions, and committed
26 frames after it was requested.

*Walking* (`streaming-measure`): eight one-chunk moves east, 13 chunks entering and 13 leaving per
move, with the 26 chunks of the two westernmost columns edited first and a save requested after
the fourth move. Each entered chunk uploaded 32,768 B. Snapshots
read back 4,260,932 B during the walk: 130 changed chunks' payloads and their flag sheets. A
chunk whose snapshot finds it unchanged costs its flag word only. Every move settled in
5 frames; loads waited at most 0.881 ms (mean
0.434), writes at most 9.13 ms, and retained payloads
peaked at 3,145,728 B. No frame or maintenance wait occurred.

| Frames | CPU p50 (ms) | p95 | p99 | max | Allocations per frame |
| --- | ---: | ---: | ---: | ---: | ---: |
| Resident, 300 frames | 0.072 | 0.113 | 0.133 | 0.377 | 119 |
| Saving, until committed | 0.076 | 0.528 | 0.554 | 0.554 | 306 |
| Walking, 8 moves | 0.129 | 0.447 | 0.530 | 0.530 | 469 |
| Resident with water, 600 frames | 0.070 | 0.100 | 0.118 | 2.01 | 119 |

Saving and walking add about 0.4 ms at the 95th percentile to a frame's CPU time and 190–350
allocations per frame; the allocations are not yet broken down by source. One resident frame
took 2.01 ms. 06–07 judge these against the frame budget.

*Under faults*: with writes failing, retention stops at its budget and the window waits
(65,536 B, the two payloads `retention-budget` allows;
65,536 B through 3 failed writes in
`failed-commits-retained`). A world snapshot draining while the window grows from radius 1 to 2
peaks at 8,616,352 requested device bytes, exactly the plan's overlap bound.

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
  declared formulas and are included in the total peak. Since 02 the resource plan refuses a
  window whose terrain-related buffers exceed `48*N` plus 64 B per chunk
  ([measured allocations](#measured-transfer-lifecycle-02)).
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

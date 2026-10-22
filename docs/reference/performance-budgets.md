# Performance budgets

**Frozen qualification contract, the measured baseline (01), the measured transfer lifecycle
(02), the material phases (03), bodies (04), streaming and checkpoints (05), the native
client's renderer and frame (06), the integrated qualification (07) and the client's loop
after it.** 00 adopted the evidence policy in
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
`3 H + 66 B + 4` words with `H` the hash table, a power of two at least `2 B` (`3 H + B + 4`
until `7f9d267` added each body's candidate count and its [pair list](#the-dense-body-pass) of
64), and the dispatch table one 256-byte entry per (substep, pass), 192 entries. At the defaults
that is 1,513,984 B of bodies, 1,179,664 B of bins (114,704 B before the pair lists) and 49,152 B
of dispatch table: 2.6 MiB in all (1.6 MiB before). The window gains one
claim word per cell (`4 N`), so terrain-related state is `29.125 N + 40 C + 64` bytes against the
`48 N + 64 C` envelope: 20,167,848 B at radius 6, 129,956,968 B at radius 16 and 504,194,664 B at
radius 32. Nothing is appended and no list has a capacity a cluster can exceed: bins have no
per-bin capacity, a body whose candidates overflow its pair list walks its bins instead (counted),
a footprint fits its workgroup's 27 × 27 cells by the radius limit, and the event and sample
buffers are bounded with their overflow counted.

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
image of 32 entries (1 MiB) and the world image a world snapshot copies into, `8 N + 144 B`
bytes (6,127,616 B at radius 6 with the default 4,096 bodies), reserved in the plan; since the
autosave cadence (after 07) the first world snapshot allocates it and later ones of the same
window reuse it. One change word per slot. During a resize the budget holds
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
took 2.01 ms. 06 measured the native frame with saving off (streaming moves the window, no
autosave runs); 07 judges these against the frame budget.

*Under faults*: with writes failing, retention stops at its budget and the window waits
(65,536 B, the two payloads `retention-budget` allows;
65,536 B through 3 failed writes in
`failed-commits-retained`). A world snapshot draining while the window grows from radius 1 to 2
peaks at 8,616,352 requested device bytes, exactly the plan's overlap bound.

## Native client (06)

**Renderer.** The `render` suite's `frame-cost` case draws generated terrain (seed 69, every rule
on) offscreen at 1920×1080 from the qualification camera with the renderer's pass timestamps, on
the tree committed as `da1c3ba` (2026-10-20 12:40 UTC; RTX 5070 Ti, Vulkan, NVIDIA 615.78.08, Linux 7.2.11). The
scene passes are the shadow map, the landscape layers and the spheres; post is bloom and tone
mapping. Medians of the measured frames:

| Window | Chunks drawn per layer | Scene (p95) | Scene unculled | Post | Encode (CPU, p95) |
| --- | --- | ---: | ---: | ---: | ---: |
| Radius 6, 169 chunks | 66, 66, 59, 66 | 0.84 ms (0.85) | 1.81 ms | 0.09 ms | 0.18 ms (0.25) |
| Radius 16, 1,089 chunks | 355, 354, 321, 348 | 4.20 ms (4.21) | 11.75 ms | 0.10 ms | 0.26 ms (0.34) |

Drawing every window chunk's four layers cost 1.81 ms and 11.75 ms; visibility derived on the
device every frame (chunk bounds, frustum test, ordered compaction into indirect draws) cut that
to 0.84 ms and 4.20 ms with pixel-identical frames (`culling-matches-unculled`). The renderer
holds 17.0 MB of static buffers and 29.9 MB of 1920×1080 targets, writes 400 B of uniforms a
frame and keeps one world bind group until the GPU owner's buffers are replaced.

**The native frame.** `cargo evidence bench --profile qualification --adapter vulkan-10de-2c05 --runs 1`
runs each frozen case once in the client's real window (`native-client bench`: a new world made
to the scenario, its commands through the client's command path for their exact ticks, the
camera on its path by tick, the overlay shown, no vsync, mailbox presentation), plus one
diagnostic run with tick timestamps. A sample is one presented frame; its CPU time is the
presenting iteration's (input, overlay, intent, the session's frame, encoding, acquisition and
submission). Idle, streaming and coupled ran on `5bf7b44` (2026-10-20 12:21 UTC); flow,
body-contact and mixed on `da1c3ba` (12:41 UTC), whose changes since `5bf7b44` are a test, the
runner's registry and a script's cursor, not the client or the benchmark. In the first run all
three attempts of flow, body-contact and mixed were rejected because KWin gave the window
1920×1052 (the 1080p display minus its panel); both reports keep every attempt. One valid run per
case: below the five the protocol asks for, so these are prequalification figures, not
qualification.

| Case | Median | p95 | p99 | Max | Presented frames | CPU median | Tick device time, median (p95) | Body pass | Upload | RSS | Baseline median / p95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| idle | 1.08 | 10.74 | 10.99 | 11.96 | 14,634 | 0.47 | 0.27 (0.55)\* | – | 16.6 kB/s | 291 MiB | 3.58 / 6.20 |
| flow | 1.11 | 10.68 | 10.87 | 12.94 | 15,426 | 0.46 | 0.23 (0.38) | – | 26.6 kB/s | 292 MiB | 11.04 / 14.56 |
| body-contact | 1.13 | 10.69 | 10.89 | 11.44 | 15,085 | 0.48 | 0.83 (1.08) | 0.64 (0.83) | 54.0 kB/s | 293 MiB | 14.68 / 18.68 |
| mixed | 1.06 | 10.69 | 10.85 | 12.41 | 14,839 | 0.45 | 0.21 (0.36) | – | 21.6 kB/s | 291 MiB | 4.59 / 12.22 |
| streaming | 1.14 | 10.69 | 10.88 | 11.61 | 14,372 | 0.47 | 0.19 (0.37) | – | 1,244 kB/s | 365 MiB | 3.57 / 6.07 |
| coupled | 1.23 | 10.63 | 10.88 | 11.93 | 16,378 | 0.45 | 0.88 (1.20) | 0.66 (0.96) | 169 kB/s | 342 MiB | 17.12 / 28.64 |

Frame times are milliseconds over the 1,800 measured ticks; every run kept 60 ticks/s and had no
frame over 16.67 ms, and the process's GPU memory peaked at 241 MiB in each. \*Idle's diagnostic
run was rejected for its window size; its tick times are shown anyway. Upload is every byte the
runtime sent to the device (commands, tick parameters, chunks entering the window); what comes
back is command results, events and selected body samples, never cells. The baseline column is the Bevy application's five-run medians
([above](#measured-legacy-baseline-01)), which sampled its main schedule rather than presented
frames.

Every case meets the qualification frame budget (p95 ≤ 16.67 ms, p99 ≤ 25 ms) in this run. The
median frame is 1.1–1.2 ms in every case, where the baseline's was 3.6–17.1 ms; the CPU's share
is about 0.46 ms, and the device's ticks cost 0.2–0.9 ms. The 95th percentile is about 10.7 ms in
every case, above the baseline's in idle and streaming (6.2 and 6.1 ms): it is set by the
acquisition waits below, not by the world's work.

**Presentation.** On this desktop (KDE Plasma, KWin Wayland, displays at 240 and 144 Hz), the
client's acquisition of a swapchain image waits a constant 10.09 ms on some presented frames:
measured around `get_current_texture` in a short idle run, on every presented frame with the
immediate present mode (about 94 frames a second) and on 369 of about 2,400 with mailbox (about
400 frames a second); submission took 0.3 ms and the present call 0.06 ms. The interval matches
no display's refresh, so it is not pacing by the compositor's frame rate; the client now prefers
mailbox. These waits are the 10.6–11 ms frames that set every case's p95; 07 measured them on
other paths ([below](#integrated-qualification-07)), and what they wait for is in [swapchain
acquisition](#swapchain-acquisition-after-07).

## Integrated qualification (07)

**Result.** The qualification profile meets its budget in every run (p95 at most 10.78 ms, p99 at
most 10.98 ms, against 16.67 and 25 ms) at 60 ticks/s, where the baseline missed it in
body-contact and coupled; the scale profile holds 60 ticks/s with no frame over 16.67 ms, where the
baseline fell to 17–30 ticks/s; the capacity profile admits radius 32 with 4,096 bodies and
reports its limits rather than truncating work. No full-grid transfer occurs, the CPU's share of a
tick fell by a factor of 30 to over 100, no frame waits for the device, at most three ticks are in
flight and selected observations are at most two ticks old. The one regression against the
baseline is the p95 of idle and streaming (10.7 against 6.2 and 6.1 ms), which the acquisition
waits of uncapped presentation explain (presentation without vsync, below); with vsync, a person's
default, they do not occur.

Executed evidence, measured on 2026-10-21 between 07:57 and 09:03 UTC with `cargo evidence bench
--profile qualification|scale|capacity --adapter vulkan-10de-2c05 --runs 5` on `eb9d8fb`, a clean
tree.

| Identity | Value |
| --- | --- |
| Source | `eb9d8fb`, clean; Rust 1.99.0; `Cargo.lock` tracked; release build |
| Application | The native client (`morbid-app`) in its real window under `native-client bench`: a new world per run made to the frozen scenario, its commands through the client's command path for their exact ticks, the camera on its path by tick, the overlay shown |
| Host | Ryzen 9 5950X (32 threads), 31 GiB, Linux 7.2.11, KDE Plasma (KWin Wayland), displays at 240 and 144 Hz; the person's desktop in use (a browser and chat clients open) |
| Adapter | RTX 5070 Ti, Vulkan, NVIDIA 615.78.08 |
| Presentation | 1920×1080 window, no vsync (mailbox), the client's full renderer and overlay |
| Protocol | Five valid runs per case plus one diagnostic run with GPU timestamps; the scenario's warm-up and measured ticks (qualification and scale 300 + 1,800, capacity 120 + 600) |
| Validity | No run contended: all-core CPU busy 1–13% and GPU utilization 0–11% before each run. Rejected and repeated for presentation: qualification mixed twice (the desktop resized and later maximized the window), scale idle once (KWin gave it 1920×1052), capacity coupled three times (paced at 16.67 ms, below) and was run again; every attempt is in the reports |
| Records | `target/evidence/07-bench-{qualification,scale,capacity}/` on the nominated machine: `report.json`, every run's `samples.csv`, `driver.json` and logs |

Frame times are in milliseconds over the measured ticks, one sample per presented frame; the
median is the median of the five run medians, and every run's p95 is listed because the
acquisition waits below come and go between runs. Process and main-thread CPU are per presented
frame over the whole measured window (every loop iteration and thread). Device time per tick and
the body pass come from the diagnostic run. Upload and readback are every byte the runtime moved,
by second. GPU memory is what nvidia-smi attributed to the process at its peak. The baseline
column is the Bevy application's five-run median and median p95 ([above](#measured-legacy-baseline-01)),
which sampled its main schedule rather than presented frames.

**Qualification** (radius 6, 64 bodies):

| Case | Valid / attempts | Median | Run p95s | Worst p99 | Max | > 16.67 ms | Ticks/s | Process CPU / main thread per frame | Iterations per frame | Device tick (body pass) | Upload / readback | RSS / GPU memory | Baseline median / p95 |
| --- | --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| idle | 5 / 5 | 1.08 | 10.69, 10.76, 10.75, 10.75, 10.66 | 10.98 | 12.29 | 0 | 60.0 | 1.01 / 0.98 | 3.6 | 0.18 | 16.2 / 76.9 KiB/s | 291 / 241 MiB | 3.58 / 6.20 |
| flow | 5 / 5 | 1.12 | 10.78, 10.73, 10.68, 1.52, 1.46 | 10.98 | 13.29 | 0 | 60.0 | 1.06 / 1.03 | 4.3 | 0.23 | 26.0 / 85.3 KiB/s | 381 / 242 MiB | 11.04 / 14.56 |
| body-contact | 5 / 5 | 1.14 | 2.03, 2.03, 2.03, 10.67, 10.65 | 10.86 | 11.89 | 0 | 60.0 | 1.52 / 1.19 | 4.4 | 0.84 (0.65) | 52.7 / 442.5 KiB/s | 452 / 300 MiB | 14.68 / 18.68 |
| mixed | 5 / 7 | 1.07 | 10.66, 10.65, 10.62, 1.26, 10.66 | 10.86 | 12.53 | 0 | 60.0 | 1.02 / 0.99 | 4.0 | 0.31 | 21.1 / 81.1 KiB/s | 380 / 242 MiB | 4.59 / 12.22 |
| streaming | 5 / 5 | 1.15 | 10.61, 10.63, 10.61, 10.62, 10.61 | 10.88 | 13.01 | 0 | 60.0 | 1.17 / 1.12 | 4.6 | 0.20 | 1214.7 / 668.0 KiB/s | 328 / 241 MiB | 3.57 / 6.07 |
| coupled | 5 / 5 | 1.24 | 2.70, 10.60, 3.11, 10.63, 10.63 | 10.93 | 65.97 | 1 | 60.0 | 1.29 / 1.26 | 5.4 | 0.88 (0.66) | 165.2 / 516.2 KiB/s | 344 / 241 MiB | 17.12 / 28.64 |

Every run of every case meets the qualification budget (p95 ≤ 16.67 ms, p99 ≤ 25 ms) and holds
60 ticks/s with nothing discarded, no frame wait and at most three ticks in flight. One frame in
all thirty runs exceeded 16.67 ms: 65.97 ms in coupled run 5 at tick 1,532, during which the loop
presented nothing and ran no tick (the presenting iteration's CPU time was 0.44 ms and the next
frame caught up three ticks); no device or maintenance wait was recorded and the cause is outside
the measured iterations, not identified. The frame medians are 0.07–0.32 of the baseline's; the
p95s are either 1.3–3.1 ms or 10.6–10.8 ms, by whether a run met the acquisition waits, so in
idle and streaming, where the baseline's p95 was 6.2 and 6.1 ms, a native run's p95 is above it
when the waits occur.

**Scale** (radius 16, 256 bodies):

| Case | Valid / attempts | Median | Run p95s | Worst p99 | Max | > 16.67 ms | Ticks/s | Process CPU / main thread per frame | Iterations per frame | Device tick (body pass) | Upload / readback | RSS / GPU memory | Baseline median / p95 |
| --- | --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| idle | 5 / 6 | 4.77 | 10.74, 10.73, 10.76, 5.72, 5.73 | 11.01 | 13.25 | 0 | 60.0 | 4.87 / 4.69 | 19.7 | 0.73 | 16.2 / 76.9 KiB/s | 360 / 498 MiB | 28.22 / 36.67 |
| flow | 5 / 5 | 4.67 | 5.61, 5.63, 5.66, 5.60, 5.63 | 5.94 | 6.23 | 0 | 60.0 | 5.25 / 4.84 | 20.8 | 0.75 | 26.0 / 85.3 KiB/s | 317 / 498 MiB | 147.86 / 160.39 |
| body-contact | 5 / 5 | 4.76 | 6.75, 6.78, 6.76, 6.77, 6.76 | 7.10 | 7.82 | 0 | 60.0 | 6.73 / 5.23 | 21.5 | 1.80 (0.98) | 52.7 / 442.5 KiB/s | 432 / 574 MiB | 205.91 / 218.14 |
| mixed | 5 / 5 | 4.55 | 5.69, 5.65, 5.68, 5.67, 5.67 | 6.01 | 6.90 | 0 | 60.0 | 5.53 / 4.74 | 20.3 | 0.84 | 21.1 / 81.1 KiB/s | 316 / 498 MiB | 170.33 / 206.86 |
| streaming | 5 / 5 | 4.71 | 5.88, 5.86, 5.94, 5.84, 5.90 | 7.12 | 8.50 | 0 | 60.0 | 5.87 / 4.90 | 18.3 | 0.77 | 3123.7 / 2086.7 KiB/s | 499 / 498 MiB | 27.03 / 52.63 |
| coupled | 5 / 5 | 4.90 | 6.90, 6.90, 6.92, 6.91, 6.87 | 7.27 | 8.80 | 0 | 60.0 | 7.53 / 5.37 | 23.2 | 1.82 (1.03) | 322.4 / 618.5 KiB/s | 465 / 574 MiB | 223.80 / 252.64 |

The scale profile holds 60 ticks/s in every case, where the baseline fell to 17–30 in every
active case and discarded 30–69 s of simulated time per run; no frame exceeded 16.67 ms, so the
scale profile meets even the qualification budget. Frame medians are 0.02–0.17 of the baseline's,
and the frame is now the renderer's (4.2 ms of scene passes at radius 16, `render` `frame-cost`).

**Capacity** (radius 32 on both axes: 4,225 chunks, 17.3 million cells; 4,096 bodies in
body-contact and coupled; 120 warm-up and 600 measured ticks, so fewer than 1,800 frames a run;
no frame-rate promise). Coupled is from a second invocation on the same commit
(`target/evidence/07-bench-capacity-coupled-2/`): in the first, three of seven attempts were
rejected as paced at 16.67 ms and the case had four valid runs. Those frames were isolated
(never two in a row) and 43 of the 46 in the worst attempt carried three catch-up ticks: with the
device saturated, frames end at the tick cadence, which equals a 60 Hz refresh interval, so the
validity rule's spike test rejects them although no compositor throttled the window. Every
attempt of both invocations is in the reports. The rule was revised afterwards so that such
isolated frames no longer count ([policy](#regression-and-improvement-policy)); the attempt counts
here are those of the rule 07 ran.

| Case | Valid / attempts | Median | Run p95s | Worst p99 | Max | > 16.67 ms | Ticks/s | Process CPU / main thread per frame | Iterations per frame | Device tick (body pass) | Upload / readback | RSS / GPU memory |
| --- | --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| idle | 5 / 5 | 18.26 | 21.39, 21.46, 21.25, 21.30, 21.26 | 21.87 | 22.52 | 2688 | 60.0 | 21.91 / 18.43 | 41.6 | 3.36 | 16.2 / 76.9 KiB/s | 359 / 1009 MiB |
| flow | 5 / 5 | 18.27 | 21.46, 21.37, 21.23, 21.34, 21.33 | 21.99 | 22.58 | 2690 | 59.9 | 21.52 / 18.46 | 40.8 | 3.35 | 26.0 / 85.3 KiB/s | 319 / 1010 MiB |
| body-contact | 5 / 5 | 26.59 | 34.34, 33.74, 42.50, 34.25, 34.26 | 43.82 | 44.78 | 1844 | 52.4 | 39.88 / 28.02 | 56.3 | 8.07 (4.86) | 46.3 / 388.8 KiB/s | 408 / 1062 MiB |
| mixed | 5 / 5 | 18.01 | 21.22, 20.99, 21.11, 21.52, 21.08 | 23.08 | 25.33 | 2724 | 59.9 | 21.45 / 18.23 | 40.2 | 3.41 | 21.1 / 81.2 KiB/s | 320 / 1009 MiB |
| streaming | 5 / 5 | 18.84 | 27.57, 26.77, 26.93, 26.78, 26.50 | 30.60 | 34.65 | 1915 | 58.8 | 26.53 / 19.67 | 37.5 | 3.29 | 6242.7 / 3980.2 KiB/s | 460 / 1778 MiB |
| coupled | 5 / 5 | 27.12 | 48.83, 48.69, 37.53, 37.56, 37.93 | 52.12 | 54.29 | 1996 | 50.9 | 38.71 / 27.50 | 54.6 | 7.83 (4.65) | 636.6 / 739.8 KiB/s | 411 / 1582 MiB |

Every workload was admitted: the window's plan fits the requested limits (the 138,444,800-byte
binding) and the 4,096-body table, the worlds became resident in 3.5–3.6 s, and gpu-world's
requested device bytes peaked at 486 MiB (967 MiB while a moving window's old and new
allocations overlap), while nvidia-smi attributed 1.0–1.8 GiB to the process with the renderer's
targets and buffers. No tick was inconsistent and nothing was truncated. With 4,096 dense bodies a
tick takes 7.8–8.1 ms of device time and the renderer most of the rest of a 27 ms frame, so the
simulation falls behind real time visibly: the runtime refuses ticks while three are in flight
(about 22,000 counted refusals a run), the clock discards 78–103 owed ticks a run beyond its
catch-up limit (counted, as `ticks_discarded`), and the 4,096 spawns, more than a tick's 256
commands, arrive late (3,072 of them) but all exist. Body samples stayed at most two ticks old.
The streaming case moves 6.1 MiB/s of chunks in and 3.9 MiB/s of changed payloads out.

**Simulation-related CPU work.** The baseline spent 2.1–9.0 ms of CPU per tick on its
simulation and 4.8–5.3 ms on bodies at qualification size, 7.6–39.6 ms and 17.0 ms at scale. On
`eb9d8fb` the CPU's share of a tick is encoding and submitting it: 53–67 µs for the material
phases at radius 6 (`cells` `activity-measurement`, idle to fully active), 211 µs with the
qualification body workload and 302 µs with the scale's (`bodies` `body-legacy-workloads`), the
session's owners 0.07 ms per frame at one tick a frame (`streaming-measure`); the physics runs on
the device in 0.2–0.9 ms per tick at qualification size and 0.7–1.8 ms at scale (the diagnostic
runs above). Per tick that is 30 to over 100 times less CPU work at qualification size (2.2 ms
against about 0.07 ms when idle, 12.2 ms against about 0.28 ms with bodies) and about 150 times
less with the scale's bodies (46.5 ms against 0.30 ms). The client's whole process uses more CPU
than these figures because without vsync its loop polls several times per presented frame and
lays out the overlay each time (below): per second it used 0.4–1.2 of a core at qualification
size and 0.9–1.4 at scale, most of it on the main thread.

**Transfers and memory.** The baseline uploaded the grid and copied it every tick: 0.60–0.85
GiB/s at qualification size, 0.76–3.15 GiB/s at scale. The native client uploads 16–53 KiB/s
with a still camera (tick parameters and commands), 165–322 KiB/s in the coupled cases and
1.2–3.1 MiB/s in the streaming cases (chunks entering the window, 32 KiB each), and reads back
77–620 KiB/s (results, events, body samples and accounting) and up to 2.0 MiB/s while streaming
(the payloads of changed chunks leaving the window); no full-grid transfer occurs. Resident memory is 291–499 MiB against the baseline's 790–1,052 MiB,
and the GPU memory attributed to the process 241–300 MiB at qualification size (the baseline's
peaked at 510 MiB) and 498–574 MiB at scale (574 MiB).

**Allocations.** About 2,000–2,900 heap allocations per presented frame at qualification size
and 8,500–10,800 at scale, nearly all on the main thread: egui's layout in every loop iteration
(about 500 each) and wgpu's command recording on the submit path (about 350 per tick, as 04
measured). Application-owned pools allocate nothing after warm-up; these are reported, not tuned.


**Presentation without vsync.** The frozen profiles present without vsync, as the baseline did,
so the client presents faster than the displays refresh. Measured on 2026-10-21 in the idle
case (900 measured ticks, quiet host; `target/evidence/07g-presentation/`), the swapchain
acquisition then waits a constant 10.09 ms on a share of the presented frames, and those
frames set the 95th percentile:

| Path | Present mode | Frames waiting > 5 ms | Their wait | Frame median / p95 / p99 | Loop iterations per presented frame |
| --- | --- | ---: | ---: | ---: | ---: |
| KWin Wayland | mailbox | 14.3% | 10.09 ms (max 10.49) | 1.08 / 10.67 / 10.81 ms | 3.8 |
| KWin Wayland, one more swapchain image | mailbox | 10.7% | 10.08 ms (max 10.54) | 1.06 / 10.61 / 10.67 ms | 4.1 |
| KWin through Xwayland | immediate (no mailbox offered) | 21.7% | 10.09 ms (max 10.33) | 1.05 / 10.72 / 10.83 ms | 2.9 |
| KWin Wayland, vsync | FIFO | 0 | – (3.59 ms median, the 240 Hz refresh) | 4.17 / 4.26 / 4.31 ms | 1.0 |

The same constant wait on two window systems, insensitive to the swapchain's length and absent
whenever presentation follows the display, points at the driver's wait for a free image when
frames outrun the compositor (an inference: neither the driver's nor KWin's source was
inspected). Since sampled: a 10 ms sleep in the driver's wait on the fence wgpu passes to each
acquisition ([swapchain acquisition](#swapchain-acquisition-after-07)). A person's client presents with vsync by default and never waits in this way; the
qualification budget holds either way.

**The loop's iterations.** Without vsync the client's loop never blocks: while both
presentation slots are busy it runs iterations that present nothing, each laying out the
overlay. In the idle case above that is 3.8 iterations, 0.98 ms of the main thread's CPU time and
about 2,000 heap allocations (nearly all egui's) per presented frame, against 0.45 ms for the
presenting iteration alone; with vsync it is one iteration, 0.61 ms and 830 allocations. Laying
out the overlay only when a frame is presented was tried and not kept: the thread stays busy
either way without vsync, and the change made scripted input timing flaky. After 07 the loop
waits instead of running such iterations ([below](#client-loop-without-vsync-after-07)).

**Saving and streaming, by owner.** `streaming-measure` on 2026-10-21 (07's gate on `eb9d8fb`'s
tree, quiet host) breaks 05's frame costs down by the session's owners
(`world_runtime::session::FrameTimes`) and by thread:

| Frames | CPU p50 / p95 (ms) | Runtime p95 | Residency p95 | Checkpoints p95 | Transfers p95 | Allocations: session thread / store workers |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Resident, 300 frames | 0.071 / 0.096 | 0.080 | 0.012 | 0.000 | 0.000 | 119 / 0 |
| Saving, until committed | 0.077 / 0.518 | 0.077 | 0.014 | 0.422 | 0.037 | 147 / 159 |
| Walking, 8 moves | 0.128 / 0.430 | 0.153 | 0.306 | 0.106 | 0.043 | 277 / 183 |

A save's extra 0.42 ms at p95 is the checkpoint owner's frame work, and walking's 0.33 ms is the
residency owner's; the store's worker threads make 160–180 of the extra allocations per frame
(encoding and redb transactions) and the session thread 30–160 (transfer submissions inside
wgpu and the owners' bookkeeping). No owner waits. These are small against the 16.67 ms frame
budget and are reported, not tuned.

## Client loop without vsync (after 07)

**Result.** Since `2c63b64` the client runs a frame only when one has work: a free presentation
slot, a due tick the device can take, a closed or lost session, or a hidden window. Otherwise it
waits in its event wait, 250 µs at a time, after asking the GPU owner what it would admit
(`GpuWorld::poll_admission`, which waits for nothing). Measured with 07's protocol, loop iterations
per presented frame fell from 3.6–5.4 to 1.0 at radius 6, from 18–23 to 1.3 at radius 16 and from
37–56 to 2.1–3.0 at radius 32; the main thread's CPU time per presented frame by 51–57%, 78–80% and
83–88%; heap allocations per presented frame by 58–70%, 88–90% and 87–92%. Frame medians are within
1.5% of 07's, inside the spread of 07's own runs, or lower (qualification coupled 1.18 against 1.24
ms, scale idle 4.63 against 4.77 ms); every qualification and scale run meets the qualification
budget, and the simulation keeps 07's rate (60 ticks/s, and 52 against 51–52 with 4,096 dense
bodies).

Executed evidence, measured on 2026-10-21 between 14:39 and 15:40 UTC with `cargo evidence bench
--profile qualification|scale|capacity --adapter vulkan-10de-2c05 --runs 5` on `2c63b64`, a clean
tree; host, adapter, presentation and protocol as in [07](#integrated-qualification-07), the
person's desktop in use as then. No run was contended and no attempt rejected. Records:
`target/evidence/loop-bench-{qualification,scale,capacity}/` on the nominated machine. 07's values
are in parentheses (capacity coupled from its second invocation); the columns are defined as in
07's tables, idle waits per presented frame added.

**qualification**

| Case | Valid / attempts | Median | Run p95s | Worst p99 | Max | Ticks/s | Main-thread CPU per frame | Process CPU | Iterations / idle waits per frame | Allocations per frame |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| idle | 5 / 5 (5 / 5) | 1.08 (1.08) | 1.38–1.47 (10.66–10.76) | 10.69 (10.98) | 12.27 (12.29) | 60.0 (60.0) | 0.48 (0.98) ms | 0.37 (0.40) cores | 1.0 / 2.6 (3.6 / 0) | 827 (1951) |
| flow | 5 / 5 (5 / 5) | 1.13 (1.12) | 1.46–1.49 (1.46–10.78) | 10.63 (10.98) | 12.44 (13.29) | 60.0 (60.0) | 0.49 (1.03) ms | 0.36 (0.52) cores | 1.0 / 2.8 (4.3 / 0) | 835 (2287) |
| body-contact | 5 / 5 (5 / 5) | 1.15 (1.14) | 2.16–2.21 (2.03–10.67) | 10.67 (10.86) | 12.42 (11.89) | 60.0 (60.0) | 0.51 (1.19) ms | 0.35 (1.23) cores | 1.0 / 3.1 (4.4 / 0) | 862 (2366) |
| mixed | 5 / 5 (5 / 7) | 1.06 (1.07) | 1.39–1.42 (1.26–10.66) | 10.65 (10.86) | 11.77 (12.53) | 60.0 (60.0) | 0.48 (0.99) ms | 0.37 (0.52) cores | 1.0 / 2.6 (4.0 / 0) | 830 (2131) |
| streaming | 5 / 5 (5 / 5) | 1.14 (1.15) | 1.47–1.48 (10.61–10.63) | 10.64 (10.88) | 12.95 (13.01) | 60.0 (60.0) | 0.50 (1.12) ms | 0.40 (0.68) cores | 1.0 / 2.9 (4.6 / 0) | 838 (2418) |
| coupled | 5 / 5 (5 / 5) | 1.18 (1.24) | 2.09–2.41 (2.70–10.63) | 10.65 (10.93) | 11.69 (65.97) | 60.0 (60.0) | 0.60 (1.26) ms | 0.69 (0.72) cores | 1.0 / 2.9 (5.4 / 0) | 860 (2864) |

**scale**

| Case | Valid / attempts | Median | Run p95s | Worst p99 | Max | Ticks/s | Main-thread CPU per frame | Process CPU | Iterations / idle waits per frame | Allocations per frame |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| idle | 5 / 5 (5 / 6) | 4.63 (4.77) | 5.51–5.54 (5.72–10.76) | 5.93 (11.01) | 6.71 (13.25) | 60.0 (60.0) | 0.97 (4.69) ms | 0.29 (0.92) cores | 1.3 / 15.6 (19.7 / 0) | 975 (9212) |
| flow | 5 / 5 (5 / 5) | 4.69 (4.67) | 5.59–5.61 (5.60–5.66) | 5.99 (5.94) | 6.77 (6.23) | 60.0 (60.0) | 1.01 (4.84) ms | 0.32 (1.08) cores | 1.3 / 15.7 (20.8 / 0) | 999 (9557) |
| body-contact | 5 / 5 (5 / 5) | 4.79 (4.76) | 6.76–6.83 (6.75–6.78) | 7.24 (7.10) | 8.35 (7.82) | 60.0 (60.0) | 1.06 (5.23) ms | 0.45 (1.28) cores | 1.3 / 17.1 (21.5 / 0) | 1090 (9972) |
| mixed | 5 / 5 (5 / 5) | 4.58 (4.55) | 5.68–5.70 (5.65–5.69) | 6.05 (6.01) | 6.75 (6.90) | 60.0 (60.0) | 0.98 (4.74) ms | 0.38 (1.16) cores | 1.3 / 15.5 (20.3 / 0) | 986 (9332) |
| streaming | 5 / 5 (5 / 5) | 4.72 (4.71) | 5.90–5.94 (5.84–5.94) | 7.19 (7.12) | 8.31 (8.50) | 60.0 (60.0) | 1.10 (4.90) ms | 0.45 (1.19) cores | 1.3 / 15.6 (18.3 / 0) | 1049 (8540) |
| coupled | 5 / 5 (5 / 5) | 4.93 (4.90) | 6.90–6.97 (6.87–6.92) | 7.47 (7.27) | 10.93 (8.80) | 60.0 (60.0) | 1.11 (5.37) ms | 0.56 (1.40) cores | 1.3 / 17.4 (23.2 / 0) | 1110 (10751) |

**capacity**

| Case | Valid / attempts | Median | Run p95s | Worst p99 | Max | Ticks/s | Main-thread CPU per frame | Process CPU | Iterations / idle waits per frame | Allocations per frame |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| idle | 5 / 5 (5 / 5) | 18.37 (18.26) | 21.28–21.67 (21.25–21.46) | 22.01 (21.87) | 22.61 (22.52) | 60.0 (60.0) | 2.22 (18.43) ms | 0.29 (1.17) cores | 2.1 / 65.4 (41.6 / 0) | 1514 (18570) |
| flow | 5 / 5 (5 / 5) | 18.43 (18.27) | 21.37–21.49 (21.23–21.46) | 21.96 (21.99) | 22.32 (22.58) | 60.0 (59.9) | 2.29 (18.46) ms | 0.45 (1.16) cores | 2.1 / 65.5 (40.8 / 0) | 1561 (18333) |
| body-contact | 5 / 5 (5 / 5) | 26.86 (26.59) | 34.45–42.00 (33.74–42.50) | 43.03 (43.82) | 56.13 (44.78) | 52.0 (52.4) | 3.61 (28.02) ms | 0.69 (1.40) cores | 3.0 / 97.2 (56.3 / 0) | 2369 (25495) |
| mixed | 5 / 5 (5 / 5) | 18.11 (18.01) | 21.18–21.32 (20.99–21.52) | 23.22 (23.08) | 24.64 (25.33) | 60.0 (59.9) | 2.31 (18.23) ms | 0.29 (1.16) cores | 2.1 / 64.3 (40.2 / 0) | 1538 (18050) |
| streaming | 5 / 5 (5 / 5) | 18.83 (18.84) | 25.39–25.81 (26.50–27.57) | 29.36 (30.60) | 30.99 (34.65) | 60.0 (58.8) | 3.38 (19.67) ms | 0.53 (1.34) cores | 2.3 / 65.9 (37.5 / 0) | 2337 (17821) |
| coupled | 5 / 5 (5 / 5) | 27.05 (27.12) | 37.54–48.65 (37.53–48.83) | 51.61 (52.12) | 82.93 (54.29) | 52.0 (50.9) | 3.56 (27.50) ms | 0.65 (1.39) cores | 2.9 / 95.6 (54.6 / 0) | 2461 (25072) |

**What the waiting costs.** A slot that frees during a wait is noticed when the wait ends. At
radius 6, where frames are shortest, the fastest frames show it: in idle the 10th percentile of
frames shorter than 5 ms rose from 0.84–0.86 to 0.94–0.95 ms and their median from 1.05 to 1.08 ms,
while their 90th percentile fell from 1.23–1.24 to 1.20–1.22 ms; the frame median is unchanged.
Each wait costs about 30 µs of main-thread CPU (pumping events and the admission poll). The 250 µs
wait was chosen from the radius-6 frame (about 1.1 ms, of which the presenting frame takes 0.45 ms
of CPU) and not tuned further.

**Acquisition waits.** The 10.09 ms waits for a swapchain image still come and go: frames of 5 ms
or more, nearly all of them these waits, were 1.9–3.4% of every qualification idle, flow,
body-contact, mixed and streaming run, none of four coupled runs and 4.2% of the fifth (07: 0 to
17% by run). No run's share reached 5%, so no p95 includes them; body-contact's run p95s (2.16–2.21
against 07's 2.03 ms in its three runs without waits) rose by them alone, since its frames shorter
than 5 ms keep their p95 (2.02 against 2.03 ms) and median (1.15 against 1.14–1.15 ms). In
alternating runs of 07's short idle scenario with the binaries of `8eb3179` and `2c63b64`, three
pairs on the same desktop (`target/evidence/loop-presentation/`), the waits hit 8.2–10.7% of the
old binary's frames and 2.5–3.3% of the new one's, with equal frame medians (1.07–1.09 ms) and p95s
of 10.6–10.8 against 1.4–1.5 ms; that the spinning provoked some of them is an inference.

**Vsync.** In two alternating pairs of the same scenario with vsync (FIFO on the 240 Hz display),
both binaries run exactly one iteration per presented frame, never wait idle, and spend 0.61–0.62
ms of main-thread CPU and 833 allocations per frame at 4.17 ms frames. The scripted runs, which
present with vsync, pass on `2c63b64`: `features` 41 checks in each of three runs, `loss` 6 and
`worlds` 5 (`target/evidence/loop-scripts/`).

**Single long frames at capacity.** The longest frame of each body-contact and coupled run takes
40–56 ms, once 83 ms (07: 35–56 ms); in body-contact it falls at ticks 360–366, as the attract
target alternates, in coupled between ticks 323 and 424. In four of these ten runs the presenting
frame itself held the main thread for 35–56 ms, most of it in the session's frame, against one such
frame (56 ms) in 07's runs; in the others the loop waited for the device, as 07's did. That work
the spinning loop spread over the polls of its idle iterations now falls into one frame is an
inference. The capacity profile's p99s are unchanged (43.0 and 51.6 against 43.8 and 52.1 ms); it
promises no frame rate.

## Checkpoint cadence (after 07)

**Result.** Since `2932764` a person's world is checkpointed every 60 ticks by default (60 to
3,600, never off; [ADR 0004](../decisions/0004-bounded-loss-by-checkpoint-cadence.md)), because
that interval bounds what a crash or a lost device loses. Under the frozen `coupled` workloads a
save every 60 ticks costs no frame time beyond the spread between runs at radius 6 or 16: the
session frame's 99th percentile CPU time was 0.50 and 0.69 ms against 0.75 and 0.65 ms with saves
off, the checkpoint owner's share 2 and 5 µs per frame. It costs storage traffic: 1.7 MB/s
written at radius 6 and 2.6 MB/s at radius 16, redb's pages and syncs included, and 0.28 MB/s
with nothing happening. A save published within 10 ticks of its request, and a crash at any frame
would have lost at most 66 and 69 ticks. Keeping the world snapshot's device image between saves
(`21674ad`) removed the one per-save cost the frame showed: at radius 16 the request allocated a
36 MB image, about 1.9 ms each, and the 99th percentile stood at 2.58 ms at `2e1a938`.

Executed evidence: `cargo evidence test gpu --suite streaming --adapter vulkan-10de-2c05` on
`2932764`, a clean tree, 2026-10-21 17:05–17:08 UTC (`target/evidence/journal-stage2-streaming/`),
cases `checkpoint-cadence-qualification` and `checkpoint-cadence-scale`; host and adapter as in
[07](#integrated-qualification-07), headless. The world is generated at seed 69 around the
scenario camera's start chunk; the case's commands are the frozen scenario's, admitted for the
tick they name (the first sphere is the driven one); the session runs one tick per frame paced at
60 Hz, 300 warm-up ticks and 900 measured ones per interval; the store is on the workspace's disk
(btrfs with zstd on LUKS on an NVMe SSD, Kingston SFYRD2000G), since the system's temporary
directory is a `tmpfs` where a sync costs nothing. Frame CPU is `WorldSession::frame_ticks(1)`, not
a native frame; storage bytes are the process's `write_bytes` in `/proc/self/io`; allocations are
the process's, the session's thread and the store's workers separately. "Most ticks exposed" is the
largest number of ticks after the latest published checkpoint at any measured frame. The created
world has no tick of its own, hence the large values with saves off.

| Workload, saves | Saves | Chunks per save (median / max) | Device copy per save (MB) | Read back (MB/s) | Written to storage (MB/s) | Ticks to publication (median / max) | Most ticks exposed | Frame CPU p50 / p95 / p99 / max (ms) | Checkpoint owner per frame (µs) | Allocations per frame (session + other threads) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| qualification coupled, off | 0 | – | – | 0 | 0 | – | 1,223 | 0.39 / 0.53 / 0.75 / 2.35 | 0 | 359 + 0 |
| qualification coupled, every 300 | 3 | 22 / 26 | 5.5 | 0.15 | 0.39 | 6 / 6 | 305 | 0.39 / 0.56 / 0.68 / 2.45 | 1 | 360 + 1.8 |
| qualification coupled, every 60 | 15 | 18 / 23 | 5.5 | 0.58 | 1.69 | 6 / 7 | 66 | 0.35 / 0.43 / 0.50 / 2.22 | 2 | 362 + 8.7 |
| qualification idle, every 60 | 15 | 0 / 5 | 5.5 | 0.02 | 0.28 | 4 / 5 | 64 | 0.15 / 0.20 / 0.28 / 2.08 | 1 | 120 + 5.4 |
| scale coupled, off | 0 | – | – | 0 | 0 | – | 1,338 | 0.43 / 0.55 / 0.65 / 2.25 | 0 | 363 + 0 |
| scale coupled, every 300 | 3 | 24 / 207 | 35.7 | 0.56 | 1.36 | 6 / 13 | 312 | 0.46 / 0.58 / 0.72 / 7.52 | 2 | 365 + 2.0 |
| scale coupled, every 60 | 15 | 19 / 80 | 35.7 | 0.89 | 2.55 | 6 / 10 | 69 | 0.43 / 0.57 / 0.69 / 2.44 | 5 | 367 + 10.4 |

The first save of a run writes what changed since the world was generated (207 chunks at radius
16), later ones what changed since the save before. Storage receives about three times the rows'
own bytes: each save is several synced redb commits (the rows, the body set, the publication, the
reclamation of what it superseded), which also make the idle workload's 0.28 MB/s with almost no
chunk changing. A store write waited up to 154 ms at radius 16 (fsync on this file system); once,
in a `check all` on 2026-10-22, one waited 5.8 s at radius 6, a save took 352 ticks to publish and
the case failed with 10 saves, then passed twice alone on the same code and in the next lane.
Retained payloads peaked at 10.6 MB against the 32 MB budget. One frame of the every-300 run at
radius 16 took 7.52 ms (2.62 ms in the same run at `2e1a938`); it is not attributed.

**In the native window** (diagnostic, not the frozen protocol, which keeps autosave off). The
frozen `coupled` scenarios through `target/release/native-client bench` on `dfeb530`'s code, three
pairs per profile alternating autosave off and `--autosave 60`, no vsync at 1920×1080, the
person's desktop otherwise idle, 2026-10-21 19:42–19:49 UTC; every run completed on its first
attempt without a window mismatch (`target/evidence/journal-window/bench/`, summarized by
`target/drafts/journal/window/summ.py`). The direct runs skip `cargo evidence bench`'s pacing and
contention checks. Frame time does not move:

| Profile | Autosave | Median (ms) | p95 | p99 | Max | Main-thread CPU per frame (ms) | Allocations per frame | Checkpoint owner per frame (µs) | Snapshot readback (MB/s) |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| qualification | off | 1.22–1.23 | 2.34–2.37 | 10.71–10.80 | 11.61–12.59 | 0.55–0.57 | 836–838 | 0.2 | 0.06 |
| qualification | every 60 | 1.22–1.23 | 2.33–2.36 | 10.73–10.75 | 11.15–12.70 | 0.55–0.56 | 854–857 | 0.5 | 0.93–0.95 |
| scale | off | 5.01–5.02 | 7.42–10.82 | 11.01–11.14 | 11.66–12.74 | 1.07–1.13 | 1071–1078 | 0.3 | 0.17 |
| scale | every 60 | 5.01–5.02 | 7.67–8.36 | 11.08–11.22 | 12.24–13.15 | 1.10–1.19 | 1119–1124 | 3.5–3.7 | 2.54–2.68 |

Every run kept 60 ticks/s with no tick discarded and no frame over 16.67 ms; the p99s are the
10.09 ms acquisition waits ([swapchain acquisition](#swapchain-acquisition-after-07)). Saving adds 18–20
allocations per presented frame at radius 6 and about 48 at radius 16 (the readback's payloads
and the workers' writes), and at radius 16 about 0.06 ms of main-thread CPU, inside the spread of
the runs without saves. The snapshot readback with autosave off is the window's evictions as the
observer circles.

**The bound under faults** (`bounded-loss`, same run, radius 1, every rule off, a marker edit
every fifth tick, paced at 60 Hz, store on disk). A device lost while an autosave was copying lost
60 ticks; a process ended right before a publication committed left the previous checkpoint, 63
ticks behind its last tick; with every storage write failing, the save was reported failed and
retrying and the exposure passed 151 ticks, then returned within the bound 12 ticks after writes
succeeded. After each, the store held exactly the marker edits of its checkpoint's ticks. The case
fails at more than 90 ticks (the interval and 30 ticks for publication).

## Capacity (after 07)

**Result.** Two costs at radius 32 grew with the window rather than with what happened in it, and
both are gone. A world at rest kept most of its tiles awake every tick because immature foliage
stayed active whether or not it could grow; since `10cf62f` (physics revision 2) foliage stays
awake only while water or mud can feed it, and the capacity profile's idle tick fell from 3.25 ms
to 0.09 ms of device time (170,151 active tiles to 54). The landscape marched its heightfield
shadows and occlusion for every vertex of every drawn chunk every frame; since `63f8ce6` the
renderer keeps that lighting per chunk vertex and relights only the chunks whose cells, or whose
neighbours toward the sun, changed, and the scene passes at radius 32 fell from 14.84 ms to
4.57 ms (from 4.30 to 1.47 ms at radius 16). With 4,096 dense bodies the body pass still took
5.12 ms of a 5.23 ms tick (8.20 ms before); since `7f9d267` the pair solver reads each body's
candidates from a list made once a substep, and it takes 2.29 ms of a 2.41 ms tick ([the dense
body pass](#the-dense-body-pass)).

Executed evidence, offscreen and headless on the RTX 5070 Ti (Vulkan, NVIDIA 615.78.08, Linux
7.2.11): `bodies` `capacity-tick-phases` and `render` `frame-cost` and `lighting-cache-matches` in
`cargo evidence check all --adapter vulkan-10de-2c05` on `d4d2d6c`, a clean tree, 2026-10-21
21:25–21:31 UTC (`target/evidence/capacity-check-all/`). The "before" tick phases ran the same
case on `b4aedf3`'s code with the case added, 2026-10-21 20:38 UTC (a draft tree; its record is
`target/drafts/capacity/phases-1.json`). The window runs are [below](#capacity-in-the-window).

**The tick at radius 32.** `capacity-tick-phases` replays the frozen capacity scenarios `idle` and
`body-contact` (seed 69, radius 32 on both axes: 4,225 chunks, every rule on, the scenario's
commands and spawns, 200 spawns a tick) on the GPU owner alone with tick timestamps, after
generating the whole window; it reports the measured ticks after the scenario's warm-up, medians
with the 95th percentile in parentheses, in device milliseconds:

| Workload | Active tiles | Tick | Activity | Material phases | Bodies |
| --- | ---: | ---: | ---: | ---: | ---: |
| idle, before | 170,151 | 3.25 (3.55) | 0.43 | 2.79 (3.10) | – |
| idle, after | 54 | 0.09 (0.24) | 0.02 | 0.05 (0.18) | – |
| body-contact (4,096 bodies), before | 169,313 | 8.20 (9.64) | 0.44 | 2.73 (3.16) | 5.01 (6.27) |
| body-contact (4,096 bodies), after | 559 | 5.23 (6.18) | 0.03 | 0.07 (0.22) | 5.12 (6.06) |
| body-contact, pair lists (`7f9d267`) | 559 | 2.41 (2.81) | 0.03 | 0.07 (0.23) | 2.29 (2.70) |

The foliage rule and its consequences are in the
[GPU contract](gpu-contract.md#rule-changes-from-the-baseline-03-and-after-07); `cells`
`dry-foliage-sleeps` shows the generated world falling asleep and poured water waking dry foliage.
The last row is the median of five runs ([the dense body pass](#the-dense-body-pass)); what
remains of the body pass is the solver's 64 passes over a crushed pile ([bodies](#bodies-04)) and
the terrain contact, and the
[dense-pile discussion](../development/virtual-office/whiteboard/discussions/dense-pile-convergence.md)
holds what would change the passes.

**The scene passes.** `frame-cost` as in [06](#native-client-06), now at radius 32 too, each
window in three lighting modes: the cache (the renderer's path), every vertex lit per frame (the
path before `63f8ce6`, kept as the comparison) and the cache relit for every drawn chunk each
frame (the sun turned 0.01° back and forth each frame, which relights every drawn chunk). Medians
with p95 in parentheses, in milliseconds of device time:

| Window | Chunks drawn per layer | Cached | Lit per vertex | Relit every frame | Unculled (cached) | Post | Lighting cache |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Radius 6, 169 chunks | 66, 66, 59, 66 | 0.46 (0.46) | 0.86 (0.88) | 0.74 (0.75) | 0.66 | 0.09 | 5.7 MB |
| Radius 16, 1,089 chunks | 355, 354, 321, 348 | 1.47 (1.48) | 4.30 (4.32) | 2.26 (2.28) | 3.21 | 0.09 | 36.8 MB |
| Radius 32, 4,225 chunks | 1,280, 1,247, 1,122, 1,275 | 4.57 (4.61) | 14.84 (14.87) | 6.87 (7.21) | 11.86 | 0.11 | 142.8 MB |

The cache is two buffers of 4 bytes per chunk vertex (65 × 65 a chunk), occlusion and sun
visibility of two layers each as 8-bit fractions, so 33,800 bytes per window chunk; it is split in
two because one buffer would exceed the 138,444,800-byte storage binding at radius 32. Each half
takes 16,900 bytes a chunk against the cell array's 32,768, so a window whose cells fit the
granted binding fits its cache. Relighting
is a compute pass dispatched indirectly from the chunk list that visibility compacts each frame: a
chunk is relit when its cells' signature (two 32-bit hashes of its cells, computed with the chunk
bounds every frame) or its residency changes, which includes a load, an eviction and a window
move, when that of a chunk within one chunk of it or up to two chunks toward the sun changes (the
shadow march reaches about 121 cells), and every drawn chunk when the window's buffers are
replaced or the sun, the shadow or occlusion strengths or the elevation scale change. A chunk out
of view keeps its flag until it is drawn again. Flames stay lit per vertex. Encode
takes 0.18–0.40 ms of CPU (p95 0.23–0.46 ms); static buffers, targets and uniforms are as in 06.

Correctness: `lighting-cache-matches` renders four cameras with the cache and lit per vertex and
compares the lighting values (not pixels): the worst difference is one 8-bit level, in at most
3,639 of a camera's values; an unchanged frame relights no chunk, one edit relit 14 chunks
(at most 27 allowed), moving the sun relights every drawn chunk, and an edit out of view is correct
once the chunk is drawn again. A change that leaves both hashes of a chunk equal would leave its
lighting stale until it or a neighbour changes again; that is not tested.

### Capacity in the window

The frozen capacity profile with 07's protocol, `cargo evidence bench --profile capacity --adapter
vulkan-10de-2c05 --runs 5` on `76376d8`, a clean tree whose code is `d4d2d6c`'s, 2026-10-21
22:01–22:14 UTC; host, adapter and presentation as in [07](#integrated-qualification-07), the
person's desktop otherwise idle (GPU utilization 2–4% before each run). No run was contended and no
attempt rejected (`target/evidence/capacity-bench-capacity/`). The values at `2c63b64`
([above](#client-loop-without-vsync-after-07)) are in parentheses; the columns are defined as there.

| Case | Valid / attempts | Median | Run p95s | Worst p99 | Max | Ticks/s | Ticks discarded | Main-thread CPU per frame | Iterations / idle waits per frame | Allocations per frame | GPU memory peak |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| idle | 5 / 5 | 5.07 (18.37) | 6.85–6.97 (21.28–21.67) | 11.56 (22.01) | 13.09 (22.61) | 60.0 (60.0) | 0 (0) | 1.36 (2.22) ms | 1.3 / 15.1 (2.1 / 65.4) | 978 (1514) | 1,236 (1,010) MiB |
| flow | 5 / 5 | 5.15 (18.43) | 6.99–7.11 (21.37–21.49) | 11.38 (21.96) | 12.94 (22.32) | 60.0 (60.0) | 0 (0) | 1.41 (2.29) ms | 1.3 / 15.5 (2.1 / 65.5) | 1008 (1561) | 1,236 (1,010) MiB |
| body-contact | 5 / 5 | 12.14 (26.86) | 14.16–14.49 (34.45–42.00) | 15.12 (43.03) | 18.66 (56.13) | 60.0 (52.0) | 0 (87–92 a run) | 2.06 (3.61) ms | 1.5 / 30.9 (3.0 / 97.2) | 1356 (2369) | 1,294–1,296 (1,060–1,070) MiB |
| mixed | 5 / 5 | 5.11 (18.11) | 8.22–8.82 (21.18–21.32) | 11.43 (23.22) | 13.74 (24.64) | 60.0 (60.0) | 0 (0) | 1.36 (2.31) ms | 1.3 / 15.4 (2.1 / 64.3) | 995 (1538) | 1,236 (1,009–1,010) MiB |
| streaming | 5 / 5 | 5.24 (18.83) | 11.30–11.36 (25.39–25.81) | 15.29 (29.36) | 17.36 (30.99) | 60.0 (60.0) | 0 (0) | 1.65 (3.38) ms | 1.3 / 16.1 (2.3 / 65.9) | 1215 (2337) | 1,777–1,778 (1,521–1,778) MiB |
| coupled | 5 / 5 | 11.34 (27.05) | 15.14–15.65 (37.54–48.65) | 17.66 (51.61) | 39.98 (82.93) | 60.0 (52.0) | 0 (80–88 a run) | 2.10 (3.56) ms | 1.5 / 30.7 (2.9 / 95.6) | 1404 (2461) | 1,582–1,836 (1,582) MiB |

The radius-32 frame median fell from 18–19 ms to 5.1–5.2 ms without dense bodies and from 27 ms
to 11–12 ms with 4,096; the dense cases keep 60 ticks/s and discard no tick, where they fell to 52.
Every case's p95 and p99 now lie inside the qualification budget's numbers (16.67 and 25 ms), which
the capacity profile does not promise. The diagnostic runs' tick device time is 0.11–0.24 ms
median without dense bodies and 5.04–5.31 ms with them (the body pass 4.85–5.17 ms).

Now that frames are shorter, the 10.09 ms acquisition waits show again
([swapchain acquisition](#swapchain-acquisition-after-07)): over 5 ms on 2–5% of the presented frames without
dense bodies and 7–12% with them, where at `2c63b64` the 18–27 ms frames hid them (none in four
cases, 3–30 a run in the dense ones); they set the 11.4–11.6 ms p99s of idle, flow and mixed.

The single long frames of dense runs remain in coupled, shorter ([window moves](#window-moves),
below, removes their cause headlessly): the longest of each run takes
32–40 ms (07 and `2c63b64`: 35–83 ms), and all 14 frames over 25 ms in the five runs fall while the
window moves a row (near ticks 230, 485–492 and 622, where the resident chunks drop from 4,225 to
4,160 and refill 16 a frame). The presenting iteration's own time in them is 1.0–1.6 ms, or
11.2–11.7 ms with an acquisition wait, so the rest of the frame waits for the device; which of its
work is not measured. Body-contact's longest frame is 18.66 ms (it was 40–56 ms).

The process's GPU memory peak rose by 226 MiB in idle, flow, mixed and body-contact, and the
lighting cache did not cause it. With wgpu's allocator report in each run's summary (`565366e`) and
capacity idle run in the window by three builds in turn (`2c63b64`, `10cf62f` before the cache, and
`565366e`; `target/drafts/capacity/memory-ab*`, nvidia-smi sampled every 0.25 s), every build holds
1,007–1,010 MiB through the measured frames, the allocator reserving 1,024 MiB in four device and
two host blocks, and every build reaches 1,236 MiB at the end, when closing the world saves it: the
save's 132 MiB world image needs a 256 MiB block of its own. `2c63b64` released the image at once,
so the bench's 2 s sampler missed that peak; since `21674ad` the image is kept until the world
drops, and the sampler catches it. The cache's two 68 MiB buffers sit in the free halves of the
blocks holding `cells` and `flow`, each 132 MiB and otherwise alone in a 256 MiB block: 136 MiB more
allocated, nothing more reserved. Coupled's and streaming's higher peaks came from window moves
allocating a second window: with moves in place they fall to 1,296 and 1,236 MiB
([below](#window-moves)). The scripted runs pass on the same tree
(`target/evidence/capacity-scripts/`, 22:14–22:15 UTC): `features` 41 checks, `loss` 6 and `worlds`
5. The qualification and scale profiles have not run since `2c63b64`.

### Window moves

Every long frame above came while the window moved a row, and every window move was a resize:
a second window's buffers allocated, every kept chunk's cells copied into them, the slots
renumbered, and every resident cell woken for the next tick, with the renderer's bind groups and
its 142.8 MB lighting cache recreated. Since `67edbc2` a move keeps the window's size and
happens in place ([GPU contract](gpu-contract.md#cell-phases-03)): the buffers, slots and
references stay, the tile records move with their chunks, and nothing wakes beyond the arriving
and leaving rows. `render` `window-move-cost` makes the residency owner's move at radius 32 on
generated terrain at rest (seed 69, every rule on, 240 ticks settled; the southern row retired in
a tick, the window moved a row north, the new row loaded 16 chunks a tick) with tick and pass
timestamps and wgpu's allocator report; before is the same case on `76376d8`'s code
(`target/drafts/capacity/move-1.json`, a draft tree), after is `check all` on `67edbc2`.

| | Before (resize) | After (move in place) |
| --- | ---: | ---: |
| The move's call, main-thread CPU | 11.22 ms | 0.40 ms |
| Cell bytes copied | 136,314,880 | 0 |
| Device bytes requested at the peak | 1,013,750,304 | 509,555,640 |
| Allocator reserved: before / after the move / settled | 1.07 / 1.88 / 1.61 GB | 1.07 / 1.07 / 1.07 GB |
| First frame after the move: wall (scene) | 15.72 ms (7.09) | 7.79 ms (7.02) |
| The two ticks after the move | 6.91, 7.11 ms, 266,240 tiles active each | 0.14, 0.14 ms, 1,452 and 934 tiles |
| Ticks 2–7 after it, active tiles | 13,555 to 1,785 | 908 to 857 |
| Scene passes over ticks 2–7 | 6.06–6.95 ms | 5.41–5.61 ms |
| Loading ticks (16 chunks each) | 0.20–0.21 ms | 0.14–0.21 ms |

At rest the case's tick takes 0.13 ms and its scene 5.90 ms (the [frame-cost](#capacity-after-07)
camera with every rule on; 5.47 ms in the draft run, so the scene varies by about 0.4 ms between
runs). The woken window had kept 1,800 tiles moving for many ticks after a move: cells asleep while
not at rest, which the move had nudged. Without the wake they stay asleep, as when the observer
stands still. The ticks after a move still hold 860–1,450 active tiles; that the retired row's wake
set them going is an inference, and the case does not record the count at rest. The renderer
relights every drawn chunk once after a move (7.02 ms of scene).

**In the window.** A first run of 07's protocol on `b766ae9` was rejected for 119 of its 124
attempts because KWin opened the window on the 1,920 × 1,080 display, where it gets 1,920 × 1,052
(`target/evidence/move-bench-*/`); the run below kept the pointer on a 2,560 × 1,440 display.
Capacity coupled's longest frame is 23.8 ms (32–40 ms by run on `76376d8`), no capacity frame
reaches 25 ms, and its frames over 16.67 ms fell from 54 to 12 in five runs; the [three
profiles](#the-three-profiles-after-the-window-move) have the rest.

### The three profiles after the window move

**Result.** With the dry foliage asleep, the terrain lighting cached and the window moved in place,
07's protocol finds every profile faster than on `2c63b64`: frame medians 0.64–0.80 ms at radius 6
(1.06–1.18), 1.64–1.89 ms at radius 16 (4.58–4.93) and 4.99–11.72 ms at radius 32 (18.11–27.05), the
dense radius-32 runs at 60 ticks/s with no tick discarded (52, and 80–92 discarded a run), and every
profile's p95 and p99 inside the qualification budget. Main-thread CPU per presented frame fell by
24–33% at radius 16 and 36–57% at radius 32; at radius 6 it is unchanged.

Executed evidence: `cargo evidence bench --profile capacity|qualification|scale --adapter
vulkan-10de-2c05 --runs 5` on `6e3f2d9`, a clean tree whose code is `67edbc2`'s, 2026-10-22
10:35–11:35 UTC; host, adapter, presentation and protocol as in [07](#integrated-qualification-07),
the person away from the desktop with the pointer on a 2,560 × 1,440 display. Every attempt was
valid and none contended (`target/evidence/move2-bench-{capacity,qualification,scale}/`). The values
on `2c63b64` ([above](#client-loop-without-vsync-after-07)) are in parentheses; GPU memory peaks
there missed the closing save's image ([capacity in the window](#capacity-in-the-window)), so radius
32's are not comparable except coupled's and streaming's, which window moves set.

**qualification**

| Case | Median | Run p95s | Worst p99 | Max | Ticks/s (min) | Ticks discarded a run | Main-thread CPU per frame | Allocations per frame | GPU memory peak |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| idle | 0.64 (1.08) | 0.98–1.12 (1.38–1.47) | 10.64 (10.69) | 11.56 (12.27) | 60.0 (60.0) | 0 (0) | 0.47 (0.48) ms | 796 (827) | 241 (241) MiB |
| flow | 0.68 (1.13) | 1.17–1.19 (1.46–1.49) | 10.66 (10.63) | 12.49 (12.44) | 60.0 (60.0) | 0 (0) | 0.48 (0.49) ms | 803 (835) | 241 (241) MiB |
| body-contact | 0.80 (1.15) | 2.08–10.59 (2.16–2.21) | 10.72 (10.67) | 12.59 (12.42) | 60.0 (60.0) | 0 (0) | 0.52 (0.51) ms | 835 (862) | 241 (241) MiB |
| mixed | 0.65 (1.06) | 1.08–1.12 (1.39–1.42) | 10.64 (10.65) | 12.45 (11.77) | 60.0 (60.0) | 0 (0) | 0.48 (0.48) ms | 799 (830) | 241 (241) MiB |
| streaming | 0.67 (1.14) | 0.86–1.18 (1.47–1.48) | 10.67 (10.64) | 11.88 (12.95) | 60.0 (60.0) | 0 (0) | 0.50 (0.50) ms | 803 (838) | 241–242 (241–242) MiB |
| coupled | 0.79 (1.18) | 1.30–1.57 (2.09–2.41) | 1.97 (10.65) | 435.74 (11.69) | 59.4 (60.0) | 0–19 (0) | 0.60 (0.60) ms | 816 (860) | 300–318 (241–300) MiB |

**scale**

| Case | Median | Run p95s | Worst p99 | Max | Ticks/s (min) | Ticks discarded a run | Main-thread CPU per frame | Allocations per frame | GPU memory peak |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| idle | 1.64 (4.63) | 1.71–1.72 (5.51–5.54) | 2.04 (5.93) | 2.23 (6.71) | 60.0 (60.0) | 0 (0) | 0.67 (0.97) ms | 835 (975) | 498 (498) MiB |
| flow | 1.66 (4.69) | 2.12–2.13 (5.59–5.61) | 2.18 (5.99) | 2.52 (6.77) | 60.0 (60.0) | 0 (0) | 0.69 (1.01) ms | 847 (999) | 498 (498) MiB |
| body-contact | 1.84 (4.79) | 3.13–3.16 (6.76–6.83) | 3.42 (7.24) | 9.52 (8.35) | 60.0 (60.0) | 0 (0) | 0.81 (1.06) ms | 889 (1090) | 556 (574) MiB |
| mixed | 1.65 (4.58) | 2.41–2.42 (5.68–5.70) | 2.81 (6.05) | 4.29 (6.75) | 60.0 (60.0) | 0 (0) | 0.69 (0.98) ms | 844 (986) | 498 (498) MiB |
| streaming | 1.66 (4.72) | 2.13–2.14 (5.90–5.94) | 2.26 (7.19) | 3.05 (8.31) | 60.0 (60.0) | 0 (0) | 0.74 (1.10) ms | 854 (1049) | 498 (498) MiB |
| coupled | 1.89 (4.93) | 3.28–3.33 (6.90–6.97) | 3.55 (7.47) | 197.43 (10.93) | 59.8 (60.0) | 0–5 (0) | 0.79 (1.11) ms | 900 (1110) | 556 (556–576) MiB |

**capacity**

| Case | Median | Run p95s | Worst p99 | Max | Ticks/s (min) | Ticks discarded a run | Main-thread CPU per frame | Allocations per frame | GPU memory peak |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| idle | 4.99 (18.37) | 6.32–7.04 (21.28–21.67) | 11.44 (22.01) | 12.16 (22.61) | 60.0 (60.0) | 0 (0) | 1.43 (2.22) ms | 977 (1514) | 1,236 (1,009–1,010) MiB |
| flow | 5.06 (18.43) | 6.62–6.66 (21.37–21.49) | 7.31 (21.96) | 8.52 (22.32) | 60.0 (60.0) | 0 (0) | 1.46 (2.29) ms | 1004 (1561) | 1,236 (1,009–1,010) MiB |
| body-contact | 11.72 (26.86) | 13.52–13.98 (34.45–42.00) | 14.75 (43.03) | 17.85 (56.13) | 60.0 (52.0) | 0 (87–92) | 1.93 (3.61) ms | 1342 (2369) | 1,294–1,296 (1,060–1,070) MiB |
| mixed | 5.04 (18.11) | 7.74–7.83 (21.18–21.32) | 8.52 (23.22) | 12.80 (24.64) | 60.0 (60.0) | 0 (0) | 1.41 (2.31) ms | 993 (1538) | 1,236 (1,009–1,010) MiB |
| streaming | 5.14 (18.83) | 6.68–6.90 (25.39–25.81) | 7.90 (29.36) | 9.73 (30.99) | 60.0 (60.0) | 0 (0) | 1.47 (3.38) ms | 1048 (2337) | 1,236 (1,521–1,778) MiB |
| coupled | 10.55 (27.05) | 14.41–14.79 (37.54–48.65) | 16.17 (51.61) | 23.80 (82.93) | 60.0 (52.0) | 0 (80–88) | 1.86 (3.56) ms | 1360 (2461) | 1,294–1,296 (1,582) MiB |

**Single long frames.** Five frames of qualification coupled's runs (56–436 ms) and one of scale
coupled's (197 ms), and in the diagnostic runs two of qualification coupled (53 and 456 ms) and one
of qualification streaming (478 ms), waited for the device: their presenting iteration took 0.4–0.8
ms, acquisition and the session's frame were short, the ticks around them took their usual device
time, and the frames after them caught up three ticks and discarded the rest (5–19 ticks in the runs
that had one). The same code on `b766ae9` had none, and in twelve alternating runs of qualification
coupled and streaming with `76376d8`'s binary (before the move) and this one, the desktop untouched
with screen locking off and other applications closed, neither binary had one: the longest frame was
10.9–12.4 ms and no tick was discarded (`target/drafts/capacity/stall-ab/`). The person reports the
screen may have locked during the run; that this caused them is an inference. The p95 of three
qualification body-contact runs is 10.6 ms: 4.6–5.4% of their frames waited 10.09 ms for a swapchain
image (3–5% in the other radius-6 cases, none at radius 16), the waits wgpu 30 removed
([swapchain acquisition](#swapchain-acquisition-after-07)).

The scripted runs pass on the same tree (`target/evidence/move-scripts/`, 11:57 UTC): `features` 41
checks, `loss` 6 and `worlds` 5.

### The dense body pass

**Result.** With 4,096 dense bodies at radius 32 the body pass took 5.02–5.10 ms of device time
of a 5.14–5.23 ms tick; since `7f9d267` it takes 2.28–2.29 ms of a 2.40–2.42 ms tick (p95 2.69–2.72
ms, was 5.99–6.11), and the bodies end exactly where they did. The pair solver's 32 passes a substep
walked each body's 27 bins and ran the generation test again in every pass, although its inputs
(where the substep's integration left the bodies, and their radii) do not change during the solve:
89% of the 37.1 million bin slots the passes visited a tick were rejected. Now `pairs_count`, which
walks the bins once a substep, records each body's candidates in a pair list of 64, and the
position, velocity and residual passes read the list ([GPU contract](gpu-contract.md#bodies-04)).
A body with more candidates walks its bins in every pass, as before, and is counted
(`BodyReport::pair_list_overflows`); the capacity workload has none.

Executed evidence, headless on the RTX 5070 Ti (Vulkan, NVIDIA 615.78.08, Linux 7.2.11):
`bodies` `capacity-tick-phases` with `54f454b`'s `gpu-suite` and `7f9d267`'s in five alternating
pairs, 2026-10-22 12:55–12:56 UTC; `body-scaling-bins`, `body-legacy-workloads` and `body-pacing`
with both binaries right after; `cargo evidence check all --adapter vulkan-10de-2c05` on `7f9d267`
(passed, 162 of 162 items, 12:58–13:04 UTC, `target/evidence/dense-check-all/`). Records of the
draft runs are in `target/drafts/dense/`.

**Where the time went.** A draft patch (`target/drafts/dense/kernel-times.patch` on `54f454b`,
`kernel-times-7f9d267.patch` on `7f9d267`, not committed) writes a timestamp after every body dispatch (`TIMESTAMP_QUERY_INSIDE_PASSES`) and adds about
0.2 ms to the pass. Medians per tick of the measured ticks, both substeps, in milliseconds:

| Kernels | Dispatches per tick | Before (`54f454b`) | After (`7f9d267`) |
| --- | ---: | ---: | ---: |
| `pairs_position` | 32 | 1.92 | 0.45 |
| `pairs_velocity` | 32 | 1.72 | 0.48 |
| `pairs_count`, `pairs_residual` | 4 | 0.19 | 0.12 |
| `contact_prepare`, `contact_apply`, `contact_response` | 6 | 1.09 | 1.18 |
| `claims_clear`, `displace_prepare`, `displace_apply` | 8 | 0.13 | 0.13 |
| Bins, integration (`body_begin`, `body_end`) and `body_observe` | 13 | 0.09 | 0.09 |
| The body pass, instrumented | 95 | 5.25–5.32 | 2.51 |

Three alternating runs of each binary, 2026-10-22 13:05 UTC; the runs agree within 0.04 ms
a row. `contact_response` takes 0.58 ms before and 0.70 ms after in every run, `contact_prepare`
0.40 and 0.37 ms, although neither kernel changed; why is not measured.

The solver's work per tick, from its diagnostics: 37,086,238 bin slots visited before and 5,318,679
after, the same 4,167,348 candidates and 91,998 contacts.

**The same computation.** Each pair's contribution is summed in fixed point or compared as the
bits of a non-negative float, so the result does not depend on the order candidates are found in,
and the list holds exactly the candidates the bins yield (`broadphase-oracle` now runs
`pairs_count` and checks that every body in contact enumerates exactly its generated candidates in
a solver pass, from its list exactly when they fit: 241 bodies of its dense cluster listed, 15
overflowed). After the capacity case, hashes of every body slot's state and configuration and of
the window's 138 MB of cells (a draft readback) are those of `54f454b`'s binary, and every
workload below reports the same candidates, contacts and residuals with both binaries. The rules
compute what they did, so the physics revision stays 2.

| Workload (`body-scaling-bins`, `body-legacy-workloads`) | Body pass median before / after | p95 before / after | Pair slots visited per tick before / after |
| --- | ---: | ---: | ---: |
| Dense 64 | 0.67 / 0.50 ms | 0.71 / 0.55 ms | 165,387 / 31,726 |
| Dense 256 | 1.40 / 0.96 ms | 1.52 / 1.14 ms | 1,286,819 / 175,158 |
| Dense 1,024 | 2.09 / 1.44 ms | 2.23 / 1.59 ms | 7,883,352 / 871,444 |
| Dense 4,096 | 8.09 / 3.78 ms | 8.44 / 4.06 ms | 73,400,940 / 6,745,527 |
| Dense 4,096, 8 + 8 passes | 9.83 / 6.79 ms | 10.31 / 7.23 ms | 99,348,557 / 64,626,905 |
| Dense 4,096, 4 + 4 passes | 11.79 / 11.12 ms | 12.27 / 11.81 ms | 129,476,422 / 123,164,485 |
| Distributed 64–4,096 | 0.24–0.93 / 0.24–0.92 ms | – | unchanged (no contacts) |
| Legacy qualification (64 bodies, radius 6) | 0.62 / 0.45 ms | 0.82 / 0.65 ms | 95,518 / 29,061 |
| Legacy scale (256 bodies, radius 16) | 0.82 / 0.60 ms | 1.21 / 0.86 ms | 648,231 / 153,228 |

With fewer passes the pile stays more compressed, and most bodies' candidates overflow their
lists, so 4 + 4 gains little; the default 16 + 16 is now the cheapest of the three by more than
before. `body-pacing` keeps 60 ticks/s with no tick discarded with both.

**Tried and not kept.** Four bodies per pair workgroup, 16 lanes each, left the position passes
at 0.46 ms and made the velocity passes slower (0.59 against 0.49 ms): with the lists, the passes
are not short of lanes. A test that skips a body's terrain probes when no vertex of its probe box
lies within reach of it (exact, since no point of a triangle lies above its highest vertex) found
about 630 of the 4,096 bodies a substep within reach of the ground, and saved 0.065 ms a tick
(`contact_prepare` 0.39 to 0.35, `contact_response` 0.69 to 0.66 ms, one run each): not worth
the second path.

**What remains.** The 64 position and velocity passes take about 0.93 ms, some 15 µs a dispatch
over 4,096 workgroups; fewer passes would change what the solver computes (a physics revision, the
[dense-pile discussion](../development/virtual-office/whiteboard/discussions/dense-pile-convergence.md)),
and 8 + 8 is slower anyway. Terrain contact takes about 1.2 ms, now half the pass, most of it in
the bodies that reach the ground, which probe it up to six times a substep in order (the
baseline's manifold); an extra probe for them costs about 0.03 ms a dispatch.

**In the window.** `cargo evidence bench --profile capacity --adapter vulkan-10de-2c05 --runs 5
--case body-contact --case coupled` on `920d097`, a clean tree whose code is `7f9d267`'s,
2026-10-22 14:25–14:29 UTC; host, adapter, presentation and protocol as in
[07](#integrated-qualification-07), the person away from the desktop with the pointer on a
2,560 × 1,440 display and the screen lock off. Every attempt was valid and none contended
(`target/evidence/dense-bench-capacity/`). The values on `6e3f2d9`
([above](#the-three-profiles-after-the-window-move)) are in parentheses, except in the diagnostic
column, where the parenthesis is the body pass of the same run:

| Case | Median | Run p95s | Worst p99 | Max | Frames > 16.67 ms | Ticks/s, discarded | Main-thread CPU per frame | Allocations per frame | Diagnostic run's tick (its body pass) | GPU memory peak |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| body-contact | 7.04 (11.72) | 10.51–10.66 (13.52–13.98) | 10.97 (14.75) | 13.50 (17.85) | 0 (1) | 60.0, 0 (60.0, 0) | 1.73 (1.93) ms | 1216 (1342) | 2.70 ms (2.56) | 1,294 (1,294–1,296) MiB |
| coupled | 7.27 (10.55) | 10.25–10.60 (14.41–14.79) | 10.97 (16.17) | 12.93 (23.80) | 0 (12) | 60.0, 0 (60.0, 0) | 1.73 (1.86) ms | 1241 (1360) | 2.58 ms (2.40) | 1,294–1,296 (1,294–1,296) MiB |

Frames alternate between about 6.5 ms, the frame that submits a tick, and about 10.5 ms, the
frame after it: two frames a tick, as on `6e3f2d9` at about 6 and 11–13 ms. No frame waited for a
swapchain image (the acquisition's p99 is at most 0.05 ms in every run), so the run p95s near
10.5 ms are that alternation, not the waits of [swapchain
acquisition](#swapchain-acquisition-after-07).

## Swapchain acquisition (after 07)

**Result.** The constant 10.09 ms waits for a swapchain image without vsync were a 10 ms sleep in
the NVIDIA driver, inside the `vkWaitForFences` that wgpu-hal 29.0.4 called after every
`vkAcquireNextImageKHR` on the fence the acquisition signals (`NativeSwapchain::acquire`; the
wait was added for frame pacing on Windows' DXGI swapchains, gfx-rs/wgpu issues 8310 and 8354).
When the acquired image is not yet free, the driver sleeps in `poll` with no file descriptor and
a 10 ms timeout instead of waiting for the fence, so the frame loses 10 ms however soon the
image frees (that it checks the fence only between such sleeps is an inference from the call
and the constant length; the driver's source was not inspected). Turning off the driver's
explicit synchronization on Wayland (`__NV_DISABLE_EXPLICIT_SYNC=1`) or not waiting on the
acquisition's fence (a draft wgpu-hal) removes every wait. Since `4485dad` the client is on wgpu
30.0.1, whose wgpu-hal waits on that fence on Windows only (30.0.0, gfx-rs/wgpu 9486) and passes
none elsewhere (30.0.1, 9855): in the same scenario no frame waits, the frame p99 is 1.05–1.06
ms against 10.64, and the sampler finds no 10 ms sleep. With vsync (FIFO), a person's default, no
frame waited before either.

Executed evidence: 07g's 15 s radius-6 idle scenario without vsync (mailbox, 1,920 × 1,080 on a
2,560 × 1,440 display), two runs of each client below, `7f9d267`'s three at 14:22–14:31 UTC with
the person away from the desktop and `4485dad`'s at 17:18–17:19 UTC with the user's go for an idle
desktop, 2026-10-22 (`target/evidence/acquire/`). A sampler (`target/drafts/acquire/sample.py`) ran
the client as its child and read its main thread's current system call
(`/proc/PID/task/PID/syscall`) every 0.2 ms, recording every call that blocked for 3 ms or more
with its arguments, what they point at and the return addresses on its stack.

**What blocked.** In the two runs of the client on wgpu 29, 751 and 695 calls (the frames whose
acquisition took over 5 ms, 669 and 616 of the measured ones, and the warm-up's) were
`poll(fds, 0, 10)`: no file descriptor and a 10 ms timeout, 9.78–10.31 ms each, and the stacks
of all but 16 held return addresses in `libnvidia-glcore.so.615.78.08` under
`wgpu::Surface::get_current_texture` and
wgpu-hal's `NativeSwapchain::acquire`, at the call after `vkAcquireNextImageKHR` that passes the
device, one fence, wait-all and the timeout (`vkWaitForFences`, by disassembly). Apart from the
start and the close of a run (shader compilation, the world's creation, the closing save), no
other system call blocked for 3 ms or more. On wgpu 30 no `poll` blocked; between the start and
the close one or two calls a run blocked 5.0–5.2 ms in the kernel's DRM syncobj wait (the
driver's explicit sync, under an `ioctl` on the render node), and nothing else for 3 ms or more.

| Client, two runs | Frames presented | Frame median | p95 | p99 | Max | Acquisitions over 5 ms | 10 ms sleeps in the driver | Main-thread CPU per frame |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| wgpu 29.0.4 (`7f9d267`) | 12,567, 13,495 | 0.64, 0.63 ms | 10.57, 1.03 ms | 10.64, 10.64 ms | 10.93, 10.97 ms | 669, 616 | 751, 695 | 0.48, 0.47 ms |
| `__NV_DISABLE_EXPLICIT_SYNC=1` | 14,506, 14,530 | 1.00, 1.00 ms | 1.17, 1.15 ms | 1.41, 1.38 ms | 2.07, 2.85 ms | 0, 0 | 0, 0 | 0.44, 0.44 ms |
| Draft wgpu-hal 29: no fence on the acquisition | 23,179, 22,474 | 0.62, 0.66 ms | 0.82, 0.83 ms | 0.98, 0.98 ms | 2.72, 2.55 ms | 0, 0 | 0, 0 | 0.53, 0.54 ms |
| wgpu 30.0.1 (`4485dad`) | 21,694, 21,776 | 0.68, 0.68 ms | 0.88, 0.88 ms | 1.06, 1.05 ms | 3.71, 3.24 ms | 0, 0 | 0, 0 | 0.58, 0.55 ms |

The draft is wgpu-hal 29.0.4 with `vkAcquireNextImageKHR` given no fence and the wait on it
removed (`target/drafts/acquire/wgpu-hal-nowait.patch`), built with a `[patch.crates-io]` entry
that was not committed. The frame's rendering
still waits on the acquisition's semaphore on the device, so only the CPU's wait goes (read in
wgpu-hal's source); wgpu 30.0.1 does the same. On wgpu 30 the frame median is 0.68 ms against the
draft's 0.62–0.66 and main-thread CPU 0.55–0.58 ms per frame against 0.53–0.54; the rest of the
upgrade differs from the draft, and what costs the difference is not measured. Without explicit
sync no frame sleeps, but the median frame is 1.00 ms against 0.64 ms; why is not measured. How
often an acquired image is still in use, and so why the waits came at 3–5% of radius 6's frames,
none of radius 16's and up to 1.9% of radius 32's, is not explained either. The 07 measurement of
the same constant wait through Xwayland fits the same fence wait; it was not sampled.

**Remedy, the user's choice (2026-10-22): the upgrade to wgpu 30** (`4485dad`, with naga 30.0.1
and egui 0.37.0), measured in all three profiles [below](#the-three-profiles-on-wgpu-30). The
alternatives were a patched wgpu-hal 29 (what the draft measured), setting
`__NV_DISABLE_EXPLICIT_SYNC` in the client (specific to one driver, and it gives up the
synchronization NVIDIA added for Wayland) or nothing, since vsync never waited.

### The three profiles on wgpu 30

**Result.** On wgpu 30.0.1 (`4485dad`) no frame of any profile waits for a swapchain image: at
radius 6 the worst p99 is 0.98–2.03 ms (10.64–10.72 on `6e3f2d9`) and every run's p95 1.64 ms or
less (up to 10.59), and the most acquisitions over 5 ms in a run of any case is 4 (up to 1,209).
Every attempt was valid, every case ran at 60 ticks/s and discarded no tick. The device's work
did not change: headless, the scene and post passes and every tick phase took what they took on
`7f9d267` (`render` `frame-cost`, `bodies` `capacity-tick-phases`), and the cells, bodies and
contacts the cases count are identical. What the upgrade costs is CPU on the client's side: in the
same session, alternating with `7f9d267`'s client at radius 16, 0.67–0.78 ms of main-thread CPU
per presented frame against 0.62–0.63, 0.83–1.22 ms of process CPU against 0.65–0.66, and 35 more
allocations per frame (883 against 848, on the main thread); the frame median is the same. The
radius-16 and radius-32 medians in the tables are 2–10% above `6e3f2d9`'s because that session was
faster: `7f9d267`'s client, run again in this one, took 1.81 ms at scale idle (1.64 then). The
median of capacity's dense cases moved from one of their two alternating frame lengths toward the
other (8.72 and 8.88 ms against 7.04 and 7.27); their mean frame is 8.35 and 8.40 ms against 8.18
and 8.24, with 2% fewer frames (1,198 and 1,191 a run against 1,223 and 1,214).

Executed evidence: `cargo evidence bench --profile capacity|qualification|scale --adapter
vulkan-10de-2c05 --runs 5` on `4485dad`, a clean tree, 2026-10-22 17:19–18:20 UTC; host, adapter,
presentation and protocol as in [07](#integrated-qualification-07), with the user's go for an
idle desktop with the pointer on a 2,560 × 1,440 display and the screen lock off. Every attempt
was valid and none contended (`target/evidence/wgpu30-bench-{capacity,qualification,scale}/`).
Values on `6e3f2d9` ([above](#the-three-profiles-after-the-window-move)), and for capacity's
body-contact and coupled on `920d097` ([the dense body pass](#the-dense-body-pass)), are in
parentheses. The same-session comparison: three alternating pairs of scale idle with `7f9d267`'s
client and `4485dad`'s, 18:23–18:27 UTC (`target/evidence/wgpu30-ab/scale-idle/`,
`target/drafts/wgpu30/ab.sh`).

**qualification**

| Case | Median | Run p95s | Worst p99 | Max | Acquisitions over 5 ms a run | Ticks/s (min) | Ticks discarded a run | Main-thread CPU per frame | Allocations per frame | GPU memory peak |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| idle | 0.65 (0.64) | 0.82–0.84 (0.98–1.12) | 0.98 (10.64) | 3.32 (11.56) | 0 (1,120–1,209) | 60.0 (60.0) | 0 (0) | 0.55 (0.47) ms | 834 (796) | 242 (241) MiB |
| flow | 0.67 (0.68) | 0.94–0.97 (1.17–1.19) | 1.19 (10.66) | 7.07 (12.49) | 0 (920–1,041) | 60.0 (60.0) | 0 (0) | 0.56 (0.48) ms | 839 (803) | 242 (241) MiB |
| body-contact | 0.78 (0.80) | 1.32–1.44 (2.08–10.59) | 1.81 (10.72) | 24.51 (12.59) | 0–1 (1,035–1,150) | 60.0 (60.0) | 0 (0) | 0.62 (0.52) ms | 854 (835) | 300 (241) MiB |
| mixed | 0.66 (0.65) | 0.89–0.92 (1.08–1.12) | 1.12 (10.64) | 2.98 (12.45) | 0 (972–1,064) | 60.0 (60.0) | 0 (0) | 0.58 (0.48) ms | 835 (799) | 242 (241) MiB |
| streaming | 0.67 (0.67) | 0.94–0.97 (0.86–1.18) | 1.18 (10.67) | 2.93 (11.88) | 0 (0–1,070) | 60.0 (60.0) | 0 (0) | 0.55 (0.50) ms | 840 (803) | 242 (241–242) MiB |
| coupled | 0.84 (0.79) | 1.48–1.64 (1.30–1.57) | 2.03 (1.97) | 8.67 (435.74) | 0–4 (0–1) | 60.0 (59.4) | 0 (0–19) | 0.65 (0.60) ms | 859 (816) | 300–318 (300–318) MiB |

**scale**

| Case | Median | Run p95s | Worst p99 | Max | Acquisitions over 5 ms a run | Ticks/s (min) | Ticks discarded a run | Main-thread CPU per frame | Allocations per frame | GPU memory peak |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| idle | 1.76 (1.64) | 2.00–2.01 (1.71–1.72) | 2.30 (2.04) | 3.46 (2.23) | 0 (0) | 60.0 (60.0) | 0 (0) | 0.79 (0.67) ms | 882 (835) | 498 (498) MiB |
| flow | 1.82 (1.66) | 2.27–2.29 (2.12–2.13) | 2.44 (2.18) | 2.97 (2.52) | 0 (0) | 60.0 (60.0) | 0 (0) | 0.75 (0.69) ms | 893 (847) | 498 (498) MiB |
| body-contact | 2.02 (1.84) | 3.16–3.26 (3.13–3.16) | 3.54 (3.42) | 9.88 (9.52) | 0–1 (0) | 60.0 (60.0) | 0 (0) | 0.83 (0.81) ms | 926 (889) | 574 (556) MiB |
| mixed | 1.79 (1.65) | 2.64–2.68 (2.41–2.42) | 2.97 (2.81) | 4.99 (4.29) | 0 (0) | 60.0 (60.0) | 0 (0) | 0.78 (0.69) ms | 891 (844) | 498 (498) MiB |
| streaming | 1.82 (1.66) | 2.30–2.33 (2.13–2.14) | 2.56 (2.26) | 3.47 (3.05) | 0 (0) | 60.0 (60.0) | 0 (0) | 0.84 (0.74) ms | 900 (854) | 498 (498) MiB |
| coupled | 2.06 (1.89) | 3.31–3.36 (3.28–3.33) | 3.62 (3.55) | 5.05 (197.43) | 0 (0) | 60.0 (59.8) | 0 (0–5) | 0.87 (0.79) ms | 936 (900) | 576 (556) MiB |

**capacity**

| Case | Median | Run p95s | Worst p99 | Max | Acquisitions over 5 ms a run | Ticks/s (min) | Ticks discarded a run | Main-thread CPU per frame | Allocations per frame | GPU memory peak |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| idle | 5.11 (4.99) | 6.50–6.68 (6.32–7.04) | 7.36 (11.44) | 8.03 (12.16) | 0–1 (0–34) | 60.0 (60.0) | 0 (0) | 1.60 (1.43) ms | 1016 (977) | 1,236 (1,236) MiB |
| flow | 5.14 (5.06) | 6.43–6.65 (6.62–6.66) | 7.45 (7.31) | 7.97 (8.52) | 0 (0) | 60.0 (60.0) | 0 (0) | 1.46 (1.46) ms | 1045 (1004) | 1,236 (1,236) MiB |
| body-contact | 8.72 (7.04) | 10.75–10.89 (10.51–10.66) | 11.21 (10.97) | 12.02 (13.50) | 0 (0) | 60.0 (60.0) | 0 (0) | 1.81 (1.73) ms | 1213 (1216) | 1,296–1,308 (1,294) MiB |
| mixed | 5.11 (5.04) | 8.04–8.15 (7.74–7.83) | 8.69 (8.52) | 12.95 (12.80) | 0 (0) | 60.0 (60.0) | 0 (0) | 1.41 (1.41) ms | 1036 (993) | 1,236 (1,236) MiB |
| streaming | 5.20 (5.14) | 6.76–6.99 (6.68–6.90) | 8.17 (7.90) | 8.96 (9.73) | 0 (0) | 60.0 (60.0) | 0 (0) | 1.73 (1.47) ms | 1083 (1048) | 1,236 (1,236) MiB |
| coupled | 8.88 (7.27) | 10.74–10.81 (10.25–10.60) | 11.25 (10.97) | 12.58 (12.93) | 0 (0) | 60.0 (60.0) | 0 (0) | 2.00 (1.73) ms | 1242 (1241) | 1,296 (1,294–1,296) MiB |

**The same session, both clients.** Scale idle, three alternating pairs:

| Client | Frames presented | Mean frame | Frame median | Acquisitions over 5 ms | Main-thread CPU per frame | Process CPU per frame | Allocations per frame |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| wgpu 29.0.4 (`7f9d267`) | 13,345–13,451 | 2.23–2.25 ms | 1.81–1.82 ms | 636–654 | 0.62–0.63 ms | 0.65–0.66 ms | 847–849 |
| wgpu 30.0.1 (`4485dad`) | 16,532–16,789 | 1.79–1.82 ms | 1.78–1.81 ms | 0 | 0.67–0.78 ms | 0.83–1.22 ms | 882–884 |

In this session wgpu 29's client waited 10 ms for 4.8% of its images at radius 16, where none of
`6e3f2d9`'s scale runs did: how often an acquired image is still in use depends on more than the
radius. Which part of the upgrade costs the CPU (wgpu-core, the driver's threads under it, or
egui 0.37's interface, whose style also lays the panels out roomier) is not measured; the frames
at radius 16 and 32 wait for the device, not for that CPU (the loop waits 4–26 times a frame).

**Other changes seen.** The `check all` on `4485dad` (163 of 163 items, 16:41–16:48 UTC,
`target/evidence/wgpu30-check-all/`) counts about 30% fewer allocations per submitted tick than
on `7f9d267` (242 against 347 in the dense body workloads, 108 against 116 for an idle sparse
tick), and wgpu 30 reports the adapter's largest buffer as 4 GiB (4,294,967,295 bytes) where wgpu
29 reported 1 TiB; the largest plan needs 138,444,800. In that run `streaming`
`checkpoint-cadence-scale` published its saves every 300 ticks in 55 ticks at the median (5–8 in
eight earlier runs); two runs of the case right after took 6. The slow saves waited 0.77–1.02 s
for their storage writes (`write_age_ms_max`, the storage worker's write of the encoded rows to
the file, which no device work is part of), as long as one of `move-check-draft`'s writes on wgpu
29 (675 ms): the disk, a btrfs at 91% with 27 GiB unallocated, not the upgrade (an inference from
where the time went). The process GPU memory peak of radius 6's body-contact is 300 MiB in every
run, as coupled's already was (241 before); the allocator's account at residency and at the end
is unchanged (256 MiB reserved, 83.1 MiB allocated against 82.1), so the difference is transient
and outside it; it is not attributed. One qualification body-contact run had a single 24.5 ms
frame and discarded no tick; it is not attributed either.

The scripted runs pass on the same tree (`target/evidence/wgpu30-scripts/`, 18:21–18:23 UTC):
`features` 41 checks, `loss` 6 and `worlds` 5. Their captures match `6e3f2d9`'s but for egui
0.37's roomier panels. The connected scenario with the native consumer (`cargo evidence run
--scenario resident-world --consumer native`, whose viewer presents through the queue since the
upgrade) passed two of three runs, 18:29–18:31 UTC (`target/evidence/wgpu30-native-scenario*`):
in the first, step 6's resting pile had a still sphere move 0.51 units across the window's
negative rebase, past the check's 0.5 (`JUMP`); the two runs after moved one 0.06 and 0.03.
Restless spheres in the pile nudge their neighbours (one moved 1.95 units in the 30 frames
before the rebase, another 0.51), and earlier passing runs reached 0.46 (07's native run on
`20499d1`) and 0.22 headless, while headless on `4485dad` the case moved one 0.02 with every
cell, body and contact count as on `7f9d267`: the margin, not the upgrade (an inference).

The margin was the check's, and `b145fc9` replaces it. Traced tick by tick, some spheres of the
crushed pile rock in a cycle of three ticks: back where they were every 30 ticks, so the 30-frame
stillness test counted them still, and up to a third of a unit away one tick later; over the ten
frames after a move, restless neighbours nudge still spheres by up to 3.1 units. A sphere now
counts as still only if it stayed within 0.02 units at every frame, and step 6 judges each window
move by the tick that carries it, each body's last sample before against its first after (no
still sphere may move 0.5), and by the median still sphere over the frames after (0.1). Headless
on `b145fc9`'s tree, 20 of 20 runs passed (`target/evidence/b145fc9-scenario-*`, 19:36–19:38 UTC)
and `check all` 163 of 163 (`target/evidence/b145fc9-check-all/`, 19:38–19:44 UTC): the largest
move of a still sphere in the tick of a move is 0.028 units, the median over the frames after
0.006 at most. Draft faults, never committed (`target/drafts/rebase-check/`), fail it in every
run: the body pass keeping the old origin (16.7 units in the tick), the origin cell one cell off
(1.45), a resume one cell east (1.04) and bodies placed a unit high (0.02 in the tick, a median of
0.45 over the frames after, which only the median catches). A sphere that rolled off to x = 132,
outside the shifted window's reach, is rightly suspended by the move (one of 30 runs before the
fix flagged it); the check now holds only the bodies the shifted window holds to their pace. The
native consumer has not run on `b145fc9`.

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
0.15 ms of a 60/120/144/165/240 Hz interval, or at least 1% of frames in stretches of two or more
consecutive frames on one interval, more than four times as many as lie 0.45–0.75 ms beside
it), and the
host was quiet (all-core CPU busy below 15% before the run, waiting up to 120 s; otherwise the run
is marked contended). A presentation rejection is retried at most twice per case and every
attempt stays in the report; a crash, timeout or simulation error is never retried.

*Pacing rule revised on 2026-10-21, after 07.* Until then the spike test counted every frame on
an interval. When the device is saturated, the client alternates long frames that run no tick
with short catch-up frames, usually of three ticks, that end near the 60 Hz tick cadence and
never two in a row. 07's first capacity coupled invocation lost three of seven attempts to
them (8, 23 and 46 isolated frames on 16.67 ms), although no compositor paced the window. A
throttle holds the window on the interval from one frame to the next: in the 14 runs `79e7ead`
rejected when KWin throttled the baseline's window, nearly every frame on 16.67 ms has a
neighbour there (at worst 128 of 131), and those frames lie a median 0.005–0.012 ms from
the interval, against 0.06–0.12 ms for the capacity frames. The spike now counts only frames in
stretches of at least two. Applied with the runner's own code to every run kept on the
nominated machine (329, from 01's pilots to 07), the revised rule changes exactly three
verdicts, accepting those capacity attempts. It still rejects the 14 throttled baseline runs
and the throttled streaming pilot that prompted the rule, each by its spike alone as well as by
its stretch, and it rejects nothing the old rule accepted. Not adopted: exempting the 60 Hz
interval because it equals the tick period (the throttled runs are on it); ignoring frames
that carry several ticks (throttled frames carry catch-up ticks too, two ticks on 277 frames of
one baseline run); a narrower band (jitter depends on the timer and the compositor). Frame
times alone still cannot tell a throttle from a loop held to its own 60 Hz tick for 30
frames; such a run is rejected, and no current workload produces one. `measurement::tests`
checks the rule against five of these runs, kept in `tools/evidence/tests/fixtures/pacing/`.

The target is no unexplained >5% regression in a required scenario's median or p95 outside the
measured noise range. This is a review threshold, not permission to regress 4.9% in every stage.
The final qualification must also show reduced simulation-related CPU work and eliminated steady
full-grid transfer versus the comparable baseline. Report ratios rather than promising an
unmeasured 2× or 10× improvement.

If renderer quality changed, keep a same-renderer/simulation-only comparison and a separate full
native-client qualification. A budget miss is reported with the responsible phase and evidence.
Do not silently lower the resolution/body count or move the target after measuring.

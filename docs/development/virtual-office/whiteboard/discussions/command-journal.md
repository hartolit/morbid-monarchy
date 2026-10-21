# A command journal for recovery shorter than the checkpoint cadence

**Supported use.** A crash, a killed process or a lost device while a world is played. Recovery
reopens the latest published checkpoint; what came after it is lost. Since
[ADR 0004](../../../../decisions/0004-bounded-loss-by-checkpoint-cadence.md) that is at most the
autosave interval (60 ticks by default) and the save in progress. A journal of the commands
after the checkpoint, replayed on recovery, could lose less.

**Source observations (at `2e1a938`).**

- A checkpoint is one completed tick's world snapshot: cells, every body's resume state (every
  field the body passes carry from one tick to the next, `world_model::resume`), the slot table
  and the physical configuration. It does not hold the material phases' activity scheduling:
  each tile's changed and persist bitmaps, from which the next tick's wake levels follow
  ([cell phases](../../../../reference/gpu-contract.md#cell-phases-03)). A reopened chunk is an
  activation, which counts every cell as edited, so all of them wake in the first tick.
- Sleep is not exact (inference from the rules, confirmed by the woken run below): a cell sleeps
  after two unchanged ticks even when a keyed draw (flow shuffles, granular stiction, phase
  rolls) could still move it later, so which cells are awake changes what the world does.
- The client runs ticks from its first frame while the window's chunks load, so the tick a chunk
  joins depends on loading time; a window move likewise.
- A command applies in the tick it is submitted in, with the tick rate its batch carries. No
  kernel reads the session epoch, wall time or a slot index for a physical decision.

**Executed (stage 1).** The `streaming` case `replay-reproduction` runs one world (radius 2,
every rule on, generated terrain) four times with the same tick-scheduled commands: 16 spheres
dropped into a pile on sand, a driven sphere with jumps, attraction and lift, every brush
(water, lava beside it, oil set on fire, ice, sand, raise, lower), and a window move away and
back, residency held so every run activates the same chunks before the same ticks. Checkpoint
C = 240, end K = 600. Run at `2e1a938` on the RTX 5070 Ti
(`cargo evidence test gpu --suite streaming`, report
`target/evidence/journal-stage1b-streaming/`):

| Comparison at K | Chunks differing | Cells differing | Bodies differing (largest distance) | Accounting over C + 1 .. K |
| --- | ---: | ---: | ---: | --- |
| The checkpoint reopened twice, commands replayed | 0 of 25 | 0 | 0 of 16 | identical, every counter |
| Reopened and replayed / ran on, every cell woken in C + 1 | 0 | 0 | 0 | identical |
| Reopened and replayed / ran on as it was | 9 | 3,470 | 10 (1.73 units) | 12 of 17 quantities differ |

The device reproduces a world exactly from the same inputs, across processes too (three runs of
the case from `gpu-suite` while drafting gave the same numbers). A reopened checkpoint differs from the world that
ran on only in which cells are awake: forcing the running world to wake every cell in C + 1
removes the whole difference. Replayed from today's checkpoints, a journal would rebuild a
different world.

The cases `checkpoint-cadence-qualification` and `-scale` measure the alternative: periodic saves
under the frozen `coupled` workloads, paced at 60 ticks/s, with the store on the workspace's
disk (btrfs with zstd on LUKS on an NVMe SSD; the system temporary directory is `tmpfs`, where
a sync costs nothing), 900 measured ticks per interval. Every 60 ticks at radius 6 (64 spheres):
a median of 18 changed chunks a save (24 at most), published within 6 ticks of its request, a
crash at any frame losing at most 65 ticks; 1.7 MB/s written to storage, redb's pages and syncs
included, and 0.25 MB/s in the idle workload; frame CPU time within the run-to-run spread of
saves off. At radius 16 (256 spheres): a median of 19 chunks a save (96 at most), within 10
ticks, at most 69 ticks exposed, 2.4 MB/s written, and the frame's 99th percentile CPU time
rose from 0.70 to 2.58 ms: the request allocates the 36 MB world image for every snapshot, which
a probe timed at 1.9 ms while drafting (stage 2 keeps the image). Every 300 ticks the exposure reached 305 and 312 ticks.

**Alternatives.** (a) A journal replayed from the last checkpoint. (b) More frequent incremental
checkpoints, the existing path. (c) Both: a journal between less frequent checkpoints.

**Assessment.** (a) and (c) need everything below before they restore the world a person saw,
and still lose what was not yet durably appended. (b) loses at most the interval and the save
in progress, through code that already handles failures, interruption and recovery, at the cost
of storage traffic. Chosen: (b), recorded in ADR 0004.

**What a journal would need.**

1. The activity state in every checkpoint: each tile's changed and persist bitmaps (16 B per
   tile, 1 KiB per chunk), copied with the world snapshot and restored without the activation's
   wake. That is a format version 2 (before release older worlds are simply refused). The alternative, waking
   every cell of the running world at each checkpoint, makes saving change the world.
2. Residency in the journal: the tick each chunk is activated or retired at and the revision it
   came from, window moves and tick-rate changes; replay holds ticks until the journaled chunks
   are staged, as `replay-reproduction` does by hand. Opening a world would also have to hold its
   first tick until the window is resident.
3. A durable append cadence (a synced commit per append batch), which bounds the loss as the
   checkpoint interval does now.
4. Replay time on recovery: up to one checkpoint interval of ticks.
5. The same adapter and driver. Replay reproduces bit for bit on the device that ran it; no
   cross-vendor identity is promised
   ([simulation correctness](../../../practices/simulation-correctness.md)), and a recovery may
   choose another adapter.

**What would change the decision.** A required loss bound below the cadence's; storage traffic
of checkpoints becoming the limiting cost (on a slower disk, or at radius 32, where a world
snapshot copies 139 MB on the device every save). The discriminating evidence is
`replay-reproduction`: once a checkpoint holds the activity state, its live-versus-reopened
drift must be zero, and the case would then assert it.

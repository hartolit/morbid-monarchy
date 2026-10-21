# 0004 — Bounded loss by checkpoint cadence, not a command journal

**Date:** 2026-10-21. **State:** adopted after the GPU transition, for the continuation note's
"no command journal" item; implemented in the commits that follow this record. Evidence,
alternatives and what a journal would need: the
[whiteboard topic](../development/virtual-office/whiteboard/discussions/command-journal.md).

## Context

A crash or a lost device loses everything after the latest published checkpoint. Until now the
client autosaved every 3,600 ticks (a minute at 60 Hz) by default and could turn autosave off, so
the loss was up to a minute, or unbounded. The requirement: lose at most a small, stated, bounded
amount of simulated time beyond the last durable point, through the normal application path,
without a CPU mirror of the world or a steady-state synchronous readback, and keep saved worlds
of format version 1 opening.

Measured at `2e1a938` (the `streaming` cases `replay-reproduction` and `checkpoint-cadence-*`):
the device reproduces a world bit for bit from the same checkpoint, commands and residency, but
a checkpoint does not hold the material phases' activity scheduling, so a world reopened from it
continues differently from the one that ran on (3,470 cells in 9 of 25 chunks and 10 of 16
spheres after 360 ticks of a busy run). Periodic saves every 60 ticks publish within 6 ticks at
radius 6 and 10 at radius 16, cost no measurable frame time at radius 6, and write 0.25 to
2.4 MB/s to storage.

## Decision

Bound the loss by the checkpoint cadence. A person's client always autosaves: every 60 ticks by
default (one second at 60 Hz), settable from 60 to 3,600 ticks, never off. A crash or a lost device
then loses at most the interval and the save in progress (published within 10 ticks of its
request in the measurements), which the loss report states. Checkpoints, recovery and the
saved-world format (version 1) are unchanged. The world snapshot keeps its device image between
saves instead of allocating it for each.

No command journal. Replaying commands from today's checkpoints would rebuild a different world;
making it exact needs a format change, journaled residency and held replay (the topic lists
them), for a loss bound only somewhat below a second.

## Alternatives

- **A journal replayed from the last checkpoint**, alone or between less frequent checkpoints.
  Needs the activity state in every checkpoint (format version 2 and a migration) or a wake of
  every cell at each checkpoint (saving would change the world), every residency change and
  tick-rate change in the journal, recovery holding ticks while chunks load, a synced append
  cadence, and the same adapter on recovery. Its loss bound is that append cadence.
- **A longer default interval** (300 ticks): about a quarter of the storage traffic, up to about
  five seconds lost.
- **Autosave a person can turn off**: the loss is then unbounded, which the requirement excludes.
  Scripted and measured runs keep their protocol of saving only when their script says.

## Consequences

Storage receives a save every second: at radius 6 a median of 18 changed chunks a save under a
busy workload, 1.7 MB/s written including redb's pages and syncs, 0.25 MB/s with nothing
happening; 2.4 MB/s at radius 16 with 256 spheres. A save whose writes fail is retried, and the
exposure grows past the bound until storage accepts writes; the overlay's save state and the
loss report show it. The frozen benchmarks still run with autosave off for comparison with 07;
the cadence's frame cost is measured separately. The [world format](../reference/world-format.md) and
[status](../product/status.md) state the bound and its evidence.

## Reversal trigger

A loss bound below the cadence's, or checkpoint traffic becoming the limiting cost (a slower disk,
radius 32). Then implement the journal's preconditions first; `replay-reproduction` is the test
that must show no drift between a reopened checkpoint and the world that ran on.

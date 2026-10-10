# 0001 — GPU physical authority

**Date:** 2026-10-10. **State:** user-requested direction; target detailed here for sprint adoption.

## Context

The current CPU automaton, body contact/deformation and renderer maintain/copy related world
representations. Keeping bodies on the CPU after terrain moves would create demand for frequent
terrain readback. The useful physical domain is a compact column world, not arbitrary 3D colliders.

## Decision

Make the GPU the owner of resident terrain and body state, including their coupled physical
updates. Rendering consumes those allocations on the same device. CPU systems provide intent and
manage residency/I/O through bounded commands, observations and immutable snapshots. Keep small CPU
references for testing only; do not maintain a production fallback or a full live mirror.

## Alternative considered

Retain CPU authority and optimize dirty uploads, avoid full copies and improve broadphase. This is
a reasonable lower-risk performance experiment and provides a useful baseline. It does not remove
the authority/representation boundary the user wants to change. Measure its relevant costs without
turning this sprint into an unrelated CPU optimization program.

A hybrid CPU-body/GPU-terrain solution is possible for limited physics, but repeated detailed
terrain interaction would cross that boundary. Adopt it only for a demonstrated isolated role with
a bounded query contract, not as the normal body solver.

## Consequences and reconsideration

GPU synchronization, layout, numerical tests, snapshot consistency and device-loss recovery become
first-class work. GPU authority does not guarantee faster frames because rendering shares the
hardware. Reconsider an algorithm's placement when measurements show its dependency/transfer cost
is lower elsewhere; do not abandon the ownership direction merely because a first kernel is slow.

Exact contracts live in architecture and the GPU/world-format references. This record does not
promise deterministic replay across vendors or recovery of unsaved GPU state.

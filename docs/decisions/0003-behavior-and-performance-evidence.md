# 0003 — Behavioral and performance evidence

**Date:** 2026-10-10. **State:** proposed policy for adoption in 00; numerical profiles freeze in 01.

## Decision

A GPU transition is accepted only with preserved supported behavior, direct shader-execution tests,
explicit lifecycle failures and measured end-to-end/resource results. Static/CPU correctness,
actual GPU correctness, interactive functionality and performance are separate evidence lanes.
Missing hardware cannot be recorded as a pass.

Preserve the baseline test intentions while allowing independently justified rule corrections.
Use exact integer accounting where the model promises conservation; use predefined physical bounds
for floating body motion. Do not require cross-vendor bitwise equality. Fix stochastic inputs
rather than relying on scheduling-dependent RNGs.

Collect actual upload/readback bytes, allocation/memory peaks, waits, frame tails and completed-tick
throughput. Freeze comparison profiles before optimization. Do not count lowered quality or reduced
workload as a physics speedup. A budget change needs its own rationale and cannot erase a failed run.

## Alternative and cost

Compiler checks, screenshots and average FPS would make development appear faster but can hide
lost material, stale state, dropped contacts, save loss and asynchronous timing mistakes. Full
integration after every small edit would instead consume excessive time. The chosen schedule uses
focused tests where dependencies are created and one final complete gate in 07.

The cost is a reusable evidence runner, deterministic fixtures and access to a real target GPU.
Keep it in a development leaf and reuse the production runtime. Do not build a separate generic
benchmark platform or copy the product's decisions into mocks.

## Reconsideration

Narrow a test or metric when it no longer supports a product guarantee and record the reason.
Do not remove it solely because the implementation fails it or an agent lacks the hardware to run it.

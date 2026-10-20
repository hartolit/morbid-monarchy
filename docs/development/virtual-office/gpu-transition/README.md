# GPU transition sprint

**00 executed on 2026-10-10; 01 on 2026-10-10 to 2026-10-12; 02 on 2026-10-13; 03 on 2026-10-17;
04 on 2026-10-17 to 2026-10-18; 05 on 2026-10-18 to 2026-10-19; 06 on 2026-10-19 to 2026-10-20.**
01 measured and preserved the baseline, extracted `world-model`
and delivered the evidence runner. 02 built the GPU boundary (`world-runtime`, `gpu-world`, the
`transfers` suite). 03 put every material rule family on the GPU with sparse scheduling (the
`cells` suite). 04 put bodies, rays and body observations on the GPU (the `bodies` suite and
`resident-world`'s first three steps). 05 added the versioned store, residency, checkpoints and
recovery (the `streaming` suite, steps 4–5). 06 made the native client (`morbid-app`, with
`world-render`) the application, ran `resident-world` in its window, measured its frame, and
removed the Bevy baseline (the `render` suite). 07 has not started. Deliver GPU-authoritative
material/body simulation and an independently
usable native application, retaining `bevy_ecs` for CPU orchestration. This is an implementation
sprint, not a planning-only program or an instruction to port one shader.

## Start here

[00](00-first-execution-prompt.md), [01](01-baseline-contracts-and-seams.md),
[02](02-gpu-runtime-and-transfer-lifecycle.md), [03](03-gpu-cell-simulation.md),
[04](04-gpu-bodies-and-interaction.md), [05](05-streaming-checkpoints-and-recovery.md) and
[06](06-native-client-and-bevy-removal.md) are complete. Assign 07 individually. Read the
[source audit](source-audit.md), [acceptance](acceptance.md) and the current [handoff](handoff.md)
before continuing a stage.

| Stage | Complete outcome |
| --- | --- |
| [00 — First execution](00-first-execution-prompt.md) | Reconciled docs, explicit scope and source identity; no runtime change. |
| [01 — Baseline and seams](01-baseline-contracts-and-seams.md) | Measured/preserved baseline, independent fixtures, framework-free model, working evidence entry point. |
| [02 — GPU runtime](02-gpu-runtime-and-transfer-lifecycle.md) | Shared context, actual GPU layout/command/query path, bounded transfers and resource lifetime. |
| [03 — Cell simulation](03-gpu-cell-simulation.md) | All current material families on GPU, conservative transfers, reproducible randomness and correct sparse work. |
| [04 — Bodies and interaction](04-gpu-bodies-and-interaction.md) | Fixed-tick coupled sphere physics, complete collision candidates and useful CPU observations. |
| [05 — Streaming and storage](05-streaming-checkpoints-and-recovery.md) | No resident CPU mirrors; safe chunk/body lifecycle, complete checkpoints, retained failed saves and import. |
| [06 — Native cutover](06-native-client-and-bevy-removal.md) | Real window/renderer/tools on shared GPU data; Bevy engine and CPU production authority removed. |
| [07 — Qualification](07-integrated-qualification-and-closure.md) | Full checks, coupled adversarial scenario, comparable hardware evidence and canonical closure. |

Each implementation stage is several coherent checkpoints, potentially several sessions. Complete
the actual work rather than reducing it to fit a context window. Resume the same stage from the
handoff when necessary. Stop after the assigned stage; do not move ahead because a successor file
exists. Dependent prompts do not execute in parallel.

## Scope and authority

User intent fixes GPU physical authority, CPU persistence/loading, native engine independence and
ECS orchestration. The package map, pass implementation and initial budget values are proposed
engineering choices. Reconcile them with real consumers/evidence before implementation, without
turning the requested direction into an open-ended debate. Record consequential alternatives in
their maintained owner; do not inherit a previous agent's confidence as a decision.

Preserve useful physical behavior, all inspected test intentions and interactive tools. Complete
save/recovery failures as part of the transition. Do not add generic 3D physics, networking, a new
scripting language, a custom shader language, or a generic workflow system. Do not alter the user's
hardware/services or saved worlds without explicit authority.

The legacy app was an isolated baseline through 05, not a second fallback in the new runtime; 06
removed it. The CPU reference may survive only in the evidence leaf with bounded
fixtures. No per-frame GPU readback/upload bridge is an acceptable intermediate “GPU authority” demo.

## Verification schedule

Run focused checks at every checkpoint, including actual GPU execution once a shader boundary is
implemented. Full workspace/feature integration, long coupled stress and the final comparative
profile run belong to 07. Missing hardware leaves the affected stage unqualified; it cannot be
made green by a silent skip. Correctness precedes performance acceptance.

Commands proposed by the docs are delivered in 01; later stages register real suites. No suite
returns pass before it exists. 01's baseline is preserved before semantic corrections and before
algorithm tuning. Budgets cannot be relaxed after a failed candidate without a recorded decision.

## Stop conditions

A proven upstream failure is repaired at its owner before dependent work continues. Record a genuine
external blocker and the last tested state; do not replace the blocked behavior with a mock or
create a new sprint automatically. The sprint is complete only when acceptance is satisfied or the
user explicitly changes scope. A final report with unrun GPU/performance lanes is honest, but it is
not “GPU transition complete.”

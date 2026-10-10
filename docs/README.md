# Morbid Monarchy documentation

The central operation is a body moving through an editable material world. The GPU should retain
that world for simulation and rendering; the CPU should manage intent, residency and durable data.
The documents below explain that boundary and how to implement it without hiding regressions.

## Find the owner

| Question | Maintained owner |
| --- | --- |
| What are we building, and what must survive a refactor? | [Vision](product/vision.md) |
| Which subsystem owns a decision, and where does code belong? | [Architecture](architecture.md) |
| What exists and what has actually been verified? | [Status](product/status.md) |
| What remains, in what order? | [Roadmap](product/roadmap.md) |
| How does an agent take an assignment through completion? | [Workflow](development/workflow.md) |
| Which engineering rules apply to this change? | [Practices](development/practices/README.md) |
| What counts as correctness and performance evidence? | [Verification evidence](development/verification-evidence.md) |
| How do CPU commands, GPU work and readbacks relate? | [GPU contract](reference/gpu-contract.md) |
| What do packed cells and saved chunks mean? | [World format](reference/world-format.md) |
| What resource and timing limits are being qualified? | [Performance budgets](reference/performance-budgets.md) |
| How do I build, run, inspect or recover the application? | [Development guide](guides/development.md) |
| Why was a durable architectural choice made? | [Decisions](decisions/README.md) |

[AGENTS.md](../AGENTS.md) is the short repository entry point, not another copy of these rules.
[Upstream sources](reference/upstream-sources.md) explain the external API constraints behind
GPU guidance; executable code and tests will own exact selected versions and layouts.

## Current assignment

[GPU transition](development/virtual-office/gpu-transition/README.md) contains prompt 00 (complete)
and seven sequential implementation stages. Its source audit records the baseline source and its
reconciliation with the checkout; it does not replace current status. The
[virtual office](development/virtual-office/README.md) owns temporary assignments, and the
[whiteboard](development/virtual-office/whiteboard/README.md) keeps unresolved cross-assignment
questions. Adopt lasting outcomes into the maintained owners before removing a completed sprint.

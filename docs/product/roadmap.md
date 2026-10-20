# Roadmap

This page owns ordered unfinished product work. Assignment details live in the active sprint;
current qualification lives in [status](status.md). A roadmap entry is not evidence of completion.

## Authorized transition

| Order | Outcome | Assignment |
| --- | --- | --- |
| 0 | Adopt the project-specific rules and reconcile the actual checkout without changing runtime behavior. | GPU transition 00 — complete (documentation only) |
| 1 | Preserve source/behavior evidence, establish repeatable measurements and extract framework-free contracts. | 01 — complete |
| 2 | Establish one shared GPU context, bounded commands/readbacks, executable layout and lifecycle contracts. | 02 — complete |
| 3 | Move all existing cell/material simulation to GPU authority, including correct sparse work scheduling. | 03 — complete |
| 4 | Move bodies and their terrain/liquid interaction to the same fixed-tick world; return usable observations. | 04 — complete |
| 5 | Make residency changes, snapshots, saves, reloads and failures safe without permanent CPU mirrors. | 05 — complete |
| 6 | Deliver the native interactive client/renderer and remove the Bevy engine production path. | 06 — complete |
| 7 | Complete integrated correctness, performance, recovery and removal checks; publish actual qualification. | 07 |

Execute through [the sprint](../development/virtual-office/gpu-transition/README.md), one prompt at a
time. Each stage can contain several tested commits and agent sessions. A failed upstream contract
blocks dependent work; it does not authorize a weaker parallel implementation.

## After qualification

Choose subsequent work from measured constraints and actual gameplay needs: rendering reuse/LOD,
larger physical workloads, alternate GPU qualification, richer interactions, or better inspection.
Do not pre-authorize CUDA/Vulkan-specific rewrites, generalized physics, multiplayer, or another
abstraction layer. Record evidence and alternatives on the whiteboard before making that choice.

Optimization is not finished merely because work moved to the GPU. A missed budget remains open
with its measured limiting stage and owner; it cannot disappear into a generic future-polish item.

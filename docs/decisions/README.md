# Architecture decisions

ADRs explain durable choices and alternatives; they do not duplicate architecture or report task
progress. A proposal is not implemented behavior. Update status and the owning reference when a
choice is adopted or superseded.

| Record | State |
| --- | --- |
| [0001 — GPU physical authority](0001-gpu-physical-authority.md) | User-requested direction; adopted in 00. Not implemented. |
| [0002 — Native composition and meaningful boundaries](0002-native-composition.md) | Bevy-engine removal and ECS retention requested by the user and adopted in 00; packages created as their consumers appear (`world-model` 01, `world-runtime` and `gpu-world` 02, `world-store` 05, `world-render` and the native `morbid-app` 06); the Bevy baseline removed in 06. |
| [0003 — Behavioral and performance evidence](0003-behavior-and-performance-evidence.md) | Engineering policy adopted in 00; profile and budget values proposed until 01 calibrates them. |

"Requested" means the user fixed the direction. "Adopted" means the sprint treats it as binding.
"Proposed" means an engineering value or boundary that evidence may still change through a
recorded decision.

A new ADR states context, actual decision/status, strongest reasonable alternative, consequences
and reversal trigger. Do not create one for every refactor, constant or shader pass. Number new
records uniquely and keep one canonical decision for a question. An agent session label is not
independent review or user approval.

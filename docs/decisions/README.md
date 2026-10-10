# Architecture decisions

ADRs explain durable choices and alternatives; they do not duplicate architecture or report task
progress. A proposal is not implemented behavior. Update status and the owning reference when a
choice is adopted or superseded.

| Record | State in this package |
| --- | --- |
| [0001 — GPU physical authority](0001-gpu-physical-authority.md) | Direction requested by the user; detailed ownership target for adoption in 00. |
| [0002 — Native composition and meaningful boundaries](0002-native-composition.md) | Bevy-engine removal / ECS retention requested; package map proposed for 01 consumer check. |
| [0003 — Behavioral and performance evidence](0003-behavior-and-performance-evidence.md) | Proposed engineering policy for adoption in 00; profile calibration belongs to 01. |

A new ADR states context, actual decision/status, strongest reasonable alternative, consequences
and reversal trigger. Do not create one for every refactor, constant or shader pass. Number new
records uniquely and keep one canonical decision for a question. An agent session label is not
independent review or user approval.

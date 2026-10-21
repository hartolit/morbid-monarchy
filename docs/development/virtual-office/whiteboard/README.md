# Whiteboard

Use this board for broader unresolved issues and design discussions that may outlive a sprint.
The active GPU transition already owns its listed defects and implementation choices; do not
duplicate every acceptance item here.

## Overview

| Topic | State | Next action | Assignment | Last evaluated |
| --- | --- | --- | --- | --- |
| [Convergence of the body solver in dense piles](discussions/dense-pile-convergence.md) | parked | Revisit if the native client's piles (06) show visible interpenetration or a rule needs contact depth. | None. | 2026-10-18 |
| [A command journal for recovery shorter than the checkpoint cadence](discussions/command-journal.md) | parked | Revisit if a loss bound below the autosave cadence is required, or checkpoint storage traffic limits a workload; the topic lists what a journal needs first. | None ([ADR 0004](../../../decisions/0004-bounded-loss-by-checkpoint-cadence.md) chose the cadence). | 2026-10-21 |

00 found nothing outside the active sprint's scope. Relevant findings discovered during
execution can be added to [issues](issues/README.md) or [discussions](discussions/README.md).

When a topic exists, this overview alone records its state (`open`, `assigned`, `parked`), next
action, real assignment and last evaluated date. Topic files own evidence/assessment, not duplicate
planning fields. Keep the board useful instead of creating speculative tasks to fill it.

## Investigate a real question

Separate source observations, executed experiments, assumptions and proposals. Identify whose
operation improves, why the current design may be insufficient, and the strongest smaller-change
or no-change alternative. State what evidence would make you reject the proposal. A title such as
“bad architecture” or “GPU optimization” is not a demonstrated defect.

For a performance topic, retain scenario/source identity and measured cost. For a semantic topic,
trace the affected rule and consumer. Add a dated actual-session contribution only when it supplies
new evidence, a counterexample or a material alternative. Do not invent participants or treat
repetition as independent support.

An assigned topic moves into a real prompt with an outcome, owner, checks and stop condition.
Unrelated questions do not block execution by default. Demonstrated blockers must name which
acceptance fails and why. Revisit parked topics only when their recorded trigger occurs.

At resolution, adopt durable knowledge into its owner, record the disposition in the reviewed
change, remove the topic/overview row and fix links. A sprint ending does not resolve every topic.
Use [prepare-sprint](prepare-sprint.md) when another sprint is requested.

# GPU transition — current handoff

Maintain this one current record; replace stale working notes rather than accumulating a session
transcript. Canonical design/status belongs in maintained docs, and raw evidence belongs in retained
artifacts. Update after each coherent checkpoint and before stopping or changing context.

## Current boundary

**State:** prepared, not executed. No runtime changes, builds, GPU runs or benchmark measurements were
performed for this documentation delivery. The first assignable action is 00 only. Creating this
file is not evidence that any acceptance item passed.

**Baseline:** supplied `morbid-monarchy-experimental.zip`, SHA-256
`3c677631e2419d7fd92afbd99818fba5fe18b8a1ba00ce12f8980d44ca871439`.
The working checkout/commit and environment must be established in 00–01.

**Accepted user direction:** GPU physical authority; CPU I/O/orchestration; shared resident rendering;
Bevy engine removed at cutover with standalone `bevy_ecs` retained. Package map, exact algorithms and
initial numeric budget values are proposed engineering choices, not measured outcomes.

**Next action:** reconcile and adopt the documentation using
[00-first-execution-prompt.md](00-first-execution-prompt.md), then stop. Do not start 01 automatically.

## Record at each stage boundary

Use the fields below with actual values after execution. “Not run,” “unavailable” and “none” are
valid when accurate; they are not a successful qualification. Do not invent commits or reviewers.

| Field | Record |
| --- | --- |
| Assigned stage / current checkpoint | Exact prompt and last completed checkpoint; do not advance because the next file exists. |
| Source state | Branch, last tested commit, uncommitted changes and unrelated work preserved. |
| Actual adoption | Owner/API/data path now canonical; consumers updated and superseded path removed. |
| Decisions / deviations | New evidence and its consequence; canonical doc/ADR updated, including any changed rule or support limit. |
| Checks | Exact commands, selected tests/counts, outcome and tested source/configuration identity. |
| Hardware / measurements | Actual adapter/backend, effective profile and retained report path; distinguish estimates from measurements. |
| Acceptance | Applicable A-items satisfied, failed or unqualified, with evidence location. |
| Data safety | Temporary store/fixture use, outstanding snapshot/save obligations and recovery implications. |
| Blockers | Concrete failure or missing external capability; owner and last tested state, not a speculative concern. |
| Next action / stop | Exact resumable task inside the same stage, or next prompt requiring a new assignment. |

For 07 closure, preserve the final accepted evidence and lasting coverage in their maintained owners
before removing this temporary handoff/sprint. Keep it active when any required gate remains open.

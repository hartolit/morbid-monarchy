# Morbid Monarchy — agent entry point

Build a coherent, measurable 2.5D world simulation. GPU authority is the intended destination;
[status](docs/product/status.md) says what actually exists. Do not confuse the two.

## Read before changing the repository

Read [vision](docs/product/vision.md), [architecture](docs/architecture.md),
[status](docs/product/status.md), then [workflow](docs/development/workflow.md).
Select the relevant [practices](docs/development/practices/README.md). For a sprint, read its
README, current prompt and handoff. Read the owning source, callers and tests before editing.
Do not reload every document for an unrelated local change.

## Non-negotiable working rules

- One owner and normal operation path per responsibility. Compose substantial boundaries;
  use modules for cohesive internals. Neither a crate per feature nor a universal engine object.
- Complete the assigned responsibility, including consumers, failures, documentation and removal.
  A smaller diff, façade, placeholder, disabled feature or relaxed assertion is not completion.
- CPU code issues intent and manages I/O. Live terrain/body authority belongs to the GPU after
  cutover. CPU test references and explicit snapshots are not alternative production authorities.
- A performance claim needs measurements, workload identity and correctness evidence. Count bytes,
  waits, allocations and frame-time tails, not just average FPS or shader execution time.
- Preserve user data and unrelated edits. Never turn read corruption into a new generated chunk,
  discard an unsuccessful save payload, or silently lower a requested workload to pass a gate.
- Work sequentially through the assigned prompt only. Stop at its boundary. Make coherent local
  commits; no automatic push, squash, rebase, hard reset or destructive cleanup.
- Distinguish observed source, executed evidence, inference and design choice. Missing GPU access
  means unqualified GPU execution, not a passing test. Do not invent measurements or reviewers.

The [virtual office](docs/development/virtual-office/README.md) owns handoffs and cleanup.
Its [whiteboard](docs/development/virtual-office/whiteboard/README.md) holds broader unresolved
questions, not excuses to leave in-scope defects. Challenge an expensive design with a concrete
alternative and a discriminating test; do not reopen settled intent merely to postpone work.

# Documentation practice

Write for a contributor trying to understand or change one operation. Explain the purpose and the
relationship that makes it understandable, then supply the contract detail needed to act. Avoid
both jargon lists and paragraphs that restate the same list.

## Give each fact a home

Vision owns intent; architecture owns responsibilities and dependency direction; status owns
implemented/qualified behavior; roadmap owns unfinished order. Reference documents own exact
cross-boundary contracts. Practices own reusable methods. Assignment progress belongs in the
current handoff, not all of these at once.

Use links instead of copying whole rules into every prompt. A prompt still needs a concrete outcome,
work order, affected owners, verification and stop condition. It is not useful when it only says
“follow the docs.” At sprint closure, move lasting knowledge into its maintained owner and remove
stale links before deleting temporary assignments.

## Explain mechanisms through consequences

“Keep the snapshot until the save succeeds” is useful because a GPU slot may already have been
reused when a write fails. Explain that consequence. Merely listing snapshot identity, durability,
retention and generations leaves the reader to infer the connection.

For a shader pass, document the input version, write owner, global synchronization boundary,
maximum work and what happens on overflow. For a solver threshold, document its units and the
failure it bounds. For a CPU observation, state its age and whether it can be used for a physical
decision. These are practical obligations, not boilerplate sections for every getter.

Use a small table or labelled text trace when it explains ordering or ownership better than prose.
Label pseudocode and proposed interfaces. Do not publish non-existent commands as runnable recipes.
Generated API/layout inventories must come from the selected source and retain their version.

## Comments and package introductions

Every workspace package needs a README beside `Cargo.toml` explaining a real use and its adjacent
owners. Library `//!` docs orient rustdoc readers. Detailed contracts belong with the relevant API;
one substantial walkthrough should not be repeated in four locations.

Comments should explain non-obvious choices: why a donor retains rejected units, why a storage
error cannot trigger generation, why a uniform offset is aligned, or why a render read follows a
physical commit. Simple accessors do not need essays. Use errors, panic conditions, units and
examples where they affect a caller's behavior.

Keep public rustdoc accurate. Do not use `ignore`/`no_run` merely to hide a failing example, or
make internals public to generate decorative API documentation. Keep original fixture bytes and
meaningful test explanations when reorganizing files.

## Evidence language

Use “source shows” for inspected implementation, “measured” for executed measurements, “expected”
for an explicit hypothesis and “target” for an acceptance value. Record source/version, command,
environment and result when they matter. An implementation agent's confident summary does not
establish a passing test. Neither a diagram nor a web source establishes this project's runtime
behavior.

Do not claim deterministic simulation because its tick is fixed, GPU residency because a buffer
exists, async behavior because a function is `async`, or portability because wgpu supports several
backends. State the demonstrated boundary and the unqualified remainder.

## Review as a reader, then verify

Follow one actual command from input through GPU application, observation and persistence. Can a
new contributor identify who owns each decision without reverse-engineering the workspace? Check
that account against the implementation and tests, then check local links, examples and formatting.

Read for subtraction as well as omissions. Remove repeated caveats and inventories that do not
help the operation. There is no paragraph, diagram, comment-density or word-count quota. A
planning document may be long when it resolves real choices; length is not evidence of depth.

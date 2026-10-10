# Development workflow

This document owns assignment scope, execution, commits and completion. Detailed engineering rules
live in [practices](practices/README.md); evidence lanes live in
[verification evidence](verification-evidence.md).

## Start an assignment

Read the repository entry point, owning docs, current source/callers/tests and the assignment's
handoff. Inspect `git status --short`. Identify the intended rule, its owner, supported consumer,
exclusions, likely edited areas, checks and stop condition. Search the whole workspace for competing
implementations and bypasses before choosing a local change.

For performance-sensitive work, state the hypothesis and comparison workload. For GPU work,
identify resource ownership, read/write phases, maximum consumption and failure outcome. Record only
unresolved choices that affect implementation; do not create a planning framework for every edit.

## Implement and review

Complete the assigned responsibility, not only named files. Update its callers, construction,
configuration, failures, fixtures and explanations. Fix necessary in-scope defects when discovered.
A final source search should find no superseded path within the adopted responsibility.

An unexpected fact can justify a different implementation. Compare the proposed alternative with
the current design and update the owner/decision before continuing. Do not quietly change product
scope, physical meanings or acceptance values. A demonstrated upstream blocker stops dependent work
until corrected; a speculative broader idea belongs on the whiteboard and does not block by default.

Use small focused tests while iterating. Before each coherent commit, inspect the diff and run the
checks capable of disproving that change. Mark broader checks not yet run. Do not label a GPU stage
complete with CPU-only evidence.

## Commit by responsibility

Keep a working interface with its consumers and tests in the same checkpoint. Stage explicit files,
review `git diff --cached`, and run `git diff --cached --check`. Keep generated captures, private
worlds, credentials and large logs out of commits. Commit a coherent tested change before starting
the next distinct change; a docs-only commit is not a substitute for committing implementation.

Local commits do not authorize pushes or history rewrites. Do not automatically amend, squash,
rebase, hard reset or force push. Preserve other contributors' work. An interrupted session records
the last tested commit, uncommitted changes and exact next action in the current handoff.

## Verification commands and availability

The current checkout has no evidence runner, tracked lockfile or pinned toolchain, and prompt 01
must establish them. Until then, use available source/document checks and accurately report
unavailable compilation. Existing package test commands are `cargo test -p monarch-engine` and
`cargo test -p spatial-lib --all-features`. `spatial-lib` has no tests, so the second command only
compiles it. These are entry points read from source, not evidence that dependency resolution or
tests have succeeded.

### Planned common entry point — delivered in 01

01 adds a `.cargo/config.toml` alias:

```toml
[alias]
evidence = "run --locked --release -p monarchy-evidence --"
```

The Rust runner owns command parsing, source/configuration identity, exit status and reports.
Commands below become maintained runnable recipes only after 01 implements and tests them. Later
prompts extend the actual registered suites; a requested missing suite is an error, not success.

| Command | Contract |
| --- | --- |
| `cargo evidence check docs` | Local links, canonical owners and document/assignment structure. |
| `cargo evidence check static` | Formatting, supported compilation/features, warning-denying Clippy/rustdoc, dependency and shader validation checks. |
| `cargo evidence test cpu` | Pure contract/reference and storage-policy tests on the production owners. |
| `cargo evidence inspect-adapter` | Enumerate actual adapters, identifiers, backend, driver, limits and features; do not guess a preferred GPU. |
| `cargo evidence test gpu --suite cells --adapter ADAPTER_ID` | Execute the named shader/lifecycle suite on the explicit adapter; report unavailable/failure distinctly. |
| `cargo evidence run --scenario resident-world --adapter ADAPTER_ID` | Execute the connected scenario through the same runtime/adapter as the app. |
| `cargo evidence bench --profile qualification --adapter ADAPTER_ID --output target/evidence/RUN` | Release measurements with frozen inputs, bounded in-flight work and raw/summary outputs. |
| `cargo evidence check all --adapter ADAPTER_ID` | Final static, CPU, all required GPU and connected recovery checks; no silent lane skipping. |

Additional GPU suite names are `transfers`, `bodies`, `streaming`, `render` and `all`. 01 provides
unsupported/not-yet-implemented failure responses, not stubs returning pass. Owners register suites
when implemented. The runner must not recursively invoke itself or hide nested Cargo failures.

### Gate policy

Prose-only work requires accuracy, local-link and structure review, plus diff whitespace checks;
it does not require rebuilding the application. Rustdoc/examples additionally need their relevant
format/doctest/documentation checks. Executable changes require focused owner tests and applicable
GPU, persistence or performance evidence.

During the authorized 00–07 sprint, run focused checks per stage and reserve the full workspace,
all supported features, long coupled stress and final hardware comparison for 07. This scheduling
exception does not postpone new shader execution, ABI tests, save-failure tests or other checks
needed to trust the next stage. If a shared change plausibly breaks an earlier stage, rerun that
stage's relevant coverage now.

The static gate must include the selected equivalents of:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked
```

Run declared additional feature combinations separately. Add dependency/advisory/license and unused
dependency checks when their pinned tools are available; missing required tools fail the static
qualification lane and are not printed as green. 01 records the supported toolchain and resolves
a compatible dependency set; the package does not invent its versions. Track `Cargo.lock` for this
application workspace. CI uses the same runner/commands, not a weaker hand-maintained copy.

## Definition of done

The supported operation works through its real consumer, ownership is coherent, adopted paths have
replaced old ones, bounds/failures are explicit and tested, and the next contributor can understand
the result from maintained docs and code. Relevant resource/latency measurements meet the gate or
are honestly marked unqualified; a failed gate is not completion.

Report what became canonical, what was removed, checks actually run with their result, evidence
location, unresolved blockers and the exact stop point. Do not flood the handoff with raw logs or
repeat the entire assignment. A session ending is not permission to start the next prompt.

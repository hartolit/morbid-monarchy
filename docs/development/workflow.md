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

Since 01 the checkout pins its toolchain, tracks `Cargo.lock` and has the evidence runner below.
Use its lanes rather than ad hoc commands: `cargo evidence test cpu` runs every test executable
and checks the result against the coverage registry, which a bare `cargo test -p PACKAGE` does
not. `spatial-lib` still has no tests of its own.

### Common entry point — delivered in 01

`.cargo/config.toml` defines the alias, and [`tools/evidence`](../../tools/evidence/README.md)
implements it:

```toml
[alias]
evidence = "run --locked --release -p monarchy-evidence --"
```

The Rust runner owns command parsing, source/configuration identity, exit status and reports.
Each command writes `report.json` and the logs of its nested commands to
`target/evidence/<time>-<command>/`, or to `--output DIR`. It exits 0 only when every required
item passed. Later prompts extend the registered suites; a requested missing suite is an error,
not success.

| Command | State | Contract |
| --- | --- | --- |
| `cargo evidence check docs` | Runnable (01) | Local links and anchors, the owner index, sprint structure, retired rule files; every workspace package has a README and is named in the architecture. |
| `cargo evidence check static` | Runnable (01; shaders since 02) | rustfmt; `cargo check` for the declared feature combinations; Clippy against the finite lint baseline; rustdoc with warnings denied; naga validation of every assembled `gpu-world` shader, with the sources written to the run directory; `cargo deny` (advisories, licenses, bans, sources) and `cargo machete` at their required versions; dependency direction on each package's own closure; toolchain/lock/alias pins; CI uses the same lanes and checker versions. |
| `cargo evidence test cpu` | Runnable (01) | Every test executable on the production owners, checked against the coverage registry. |
| `cargo evidence inspect-adapter` | Runnable (01) | Enumerates adapters with stable identifiers, backend, driver, limits and features; picks none. |
| `cargo evidence test gpu --suite transfers --adapter ADAPTER_ID` | Runnable (02) | Runs each case of the suite in its own `gpu-suite` process on the explicit adapter, with a time limit, and checks the executed cases against the registry; a missing adapter is unavailable, a failed or missing case fails. |
| `cargo evidence test gpu --suite cells --adapter ADAPTER_ID` | Runnable (03) | The same for the material phases: the CPU reference, dense and sparse execution, the deliberate unmatched-transfer specimen and the activity measurement. |
| `cargo evidence test gpu --suite bodies` (and `streaming`, `render`) | Fails until the owning stage registers the suite | The same, for the suites of 04–06. |
| `cargo evidence run --scenario resident-world --adapter ADAPTER_ID` | Fails until 04–05 | Executes the connected scenario through the same runtime/adapter as the app. |
| `cargo evidence bench --profile qualification --adapter ADAPTER_ID --output target/evidence/RUN` | Runnable (01, baseline driver) | Release measurements of the frozen scenarios, with run validation and raw/summary outputs. `--case NAME` (repeatable) and `--runs N` select a subset for a focused comparison. |
| `cargo evidence check all --adapter ADAPTER_ID` | Fails until every lane exists | Docs, static, CPU, all GPU suites and the connected scenario; no silent skipping. |

The GPU suite names are `transfers` (02), `cells` (03), `bodies` (04), `streaming` (05), `render`
(06) and `all`. A suite fails explicitly until its owner registers it. The runner never invokes
itself recursively or hides a nested Cargo failure.

### Continuous integration

[`.github/workflows/evidence.yml`](../../.github/workflows/evidence.yml) runs `check docs`,
`check static` and `test cpu` on a hosted Ubuntu runner, installs `cargo-deny` 0.20.2 and
`cargo-machete` 0.9.2, and keeps the reports. `check static` fails if the workflow stops running a
lane or installs other checker versions. Hosted runners have no GPU, so the GPU lanes and
benchmarks run by hand on the nominated hardware, and the stage handoff records the command,
adapter and report path. No self-hosted runner is assigned.

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

Run declared additional feature combinations separately. `check static` runs `cargo deny --locked
check` against [`deny.toml`](../../deny.toml) and `cargo machete`, and requires the versions in
`tools/evidence/src/checks/static_lane.rs`; a missing or different version fails the lane and is
not printed as green. Assembled-shader validation is a required item of the same lane since 02.
The legacy Bevy shaders under `crates/morbid-app/assets` are validated only when Bevy creates
their pipelines; they are exempt until 06 removes them. 01
records the supported toolchain and resolves a compatible dependency set; the package does not
invent its versions. Track `Cargo.lock` for this
application workspace. CI uses the same runner/commands, not a weaker hand-maintained copy.

## Definition of done

The supported operation works through its real consumer, ownership is coherent, adopted paths have
replaced old ones, bounds/failures are explicit and tested, and the next contributor can understand
the result from maintained docs and code. Relevant resource/latency measurements meet the gate or
are honestly marked unqualified; a failed gate is not completion.

Report what became canonical, what was removed, checks actually run with their result, evidence
location, unresolved blockers and the exact stop point. Do not flood the handoff with raw logs or
repeat the entire assignment. A session ending is not permission to start the next prompt.

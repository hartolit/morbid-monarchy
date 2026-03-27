# Workflow: @audit (Performance & Architecture)

When the user triggers `@audit` (or requests a performance/architecture review), perform the following:

1. **Analyze Context & Scope:** Read the requested files or globally scan for hot-paths, unnecessary allocations, ownership mistakes, leaky abstractions, or domain violations.
2. **Draft the Report:** Generate a markdown report detailing the findings.
   - Write this report into the `doc/reports/` directory using the naming convention `YYYY-MM-DD-target-report.md`.
   - The report MUST include:
     - **Scope Executed**: What was analyzed.
     - **Findings**: Specific bottlenecks or architectural violations.
     - **Evidence Validity**: Which signals are valid/invalid and why (e.g., all-zero metrics, missing capture window, inactive instrumentation).
     - **Proposed Fixes**: How to resolve them without breaking `doc/architecture.md` constraints.
     - **Regressions to Avoid**: Explicit notes on what *not* to do (e.g., "Do not push runtime I/O into `mm-core`" or "Do not replace typed config with scattered constants").
     - **Root-Cause Hypothesis Ranking**: Ranked confidence with falsification tests.
3. **Generate Action Plan:** Output a strict checklist (`[ ]`) based on the report's proposed fixes.
   - Include explicit **Module Ownership Map** and **Feature-Flag Plan** for diagnostics/debug changes.
4. **Wait for Approval:** Do NOT write code yet. Wait for the user to review the report and say "Execute".
5. **Execution:** Systematically execute the checklist.
6. **Definition of Done:** Before claiming completion, verify:
   - Cargo checks pass for affected crates.
   - Implemented diagnostics or tooling are complete (no dead code paths or half-wired outputs).
   - Captured metrics are non-zero/sane in at least one known-active scenario.
7. **Documentation Update:** Append a summary of the audit and fixes to `doc/CHANGELOG.md`.
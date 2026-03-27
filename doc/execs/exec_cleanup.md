# Workflow: @cleanup

When the user triggers `@cleanup`, perform the following:

1. **Analyze Context:** Scan the current working context for:
   - Workspace boundary violations (e.g., bootstrap, logging, or boundary I/O leaking into `mm-core`).
   - Magic numbers / hardcoded values.
   - Dead code, empty files, or unused imports.
   - Swallowed errors (e.g., `Result` ignored without an explicit rationale).
2. **Generate Strict Plan:** Output a Markdown checklist (`[ ]`) detailing exactly what files will be touched and what will be deleted. State which boundaries from `architecture.md` are being enforced.
   - Include a **Containment Map** (where cleanup logic belongs and where it must not spread).
   - Include **Release Safety** notes for any debug/diagnostic cleanup so release behavior is explicit.
3. **Wait for Approval:** Do NOT write code yet. Wait for the user to say "Execute".
4. **Execution:** Systematically execute the checklist.
5. **Definition of Done:** Before claiming completion, verify:
   - Cargo checks pass for affected crates.
   - No partial cleanups remain (no dead modules, dangling config, or placeholder TODOs in delivered scope).
   - Deletions are safe and validated by deterministic references.
6. **Documentation Update:** Append a brief summary to `doc/CHANGELOG.md`.
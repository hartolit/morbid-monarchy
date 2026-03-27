# Workflow: @restructure

When the user triggers `@restructure` (or asks to refactor a specific module/crate), perform the following:

1. **Identify God Files/Slop:** Analyze the target for files handling too many concerns or flat directories lacking cohesion.
2. **Design Hierarchy:** Plan a clean directory and module structure strictly adhering to the crate's domain (`core`, app, shared vocabulary, or protocol).
3. **File Strategy (Split vs. Consolidate):** Plan the breakdown based on semantic cohesion.
   - Separate distinct sub-domains into their own files or modules.
   - Consolidate single-concept features into a single file to avoid fragmentation.
   - Ensure module declarations and file layout compile cleanly after the move.
4. **Generate Strict Plan:** Output the proposed file tree and a step-by-step checklist (`[ ]`).
   - Include a **Containment Map** to avoid spreading responsibilities across unrelated modules.
   - Include **Release Safety** notes for any debug/diagnostic paths touched by the restructure.
5. **Wait for Approval:** Do NOT write code yet. Wait for the user to review the tree and say "Execute".
6. **Execution:**
   - Create directories and module files as required by the chosen layout.
   - Migrate logic while preserving public APIs or clearly documenting any intentional boundary shifts.
   - Fix all import paths and module declarations.
   - Ruthlessly delete old ghost files.
7. **Definition of Done:** Before claiming completion, verify:
   - Cargo checks pass for affected crates.
   - No half-migrated modules or dead re-export paths remain.
   - Behavior parity is confirmed for touched flows, or intentional behavior changes are explicitly documented.
8. **Documentation Update:** Update `doc/architecture.md` if boundary rules shifted, and append a summary to `doc/CHANGELOG.md`.
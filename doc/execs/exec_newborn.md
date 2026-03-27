# Workflow: @newborn

When the user triggers `@newborn` in an empty repository, perform the following initialization:

1. **Scaffold Cargo Workspace:**
   - Create the root `Cargo.toml` with `[workspace]` members.
   - Generate `crates/*-core` and one thin application crate such as `crates/*-client`, `crates/*-cli`, or `crates/*-server`, based on the requested runtime.
   - Add `crates/*-shared` or `crates/*-protocol` only if a concrete cross-crate boundary already exists.
2. **Documentation Setup:**
   - Create the `doc/` directory.
   - Populate `architecture.md`, `CHANGELOG.md`, and the `exec_*.md` files with pure Rust workspace guidance.
3. **Generate Plan:** Output the created file tree and a strict checklist (`[ ]`).
   - Include a **Containment Map** for crate/domain ownership.
   - Include **Release Safety** notes for any default debug/diagnostic scaffolding.
4. **Wait for Approval:** Do NOT proceed with additional implementation beyond scaffold plan until user confirms foundation.
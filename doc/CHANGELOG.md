# Agentic Changelog

## 2026-03-28
- Reset the repository to a refreshed barebones Cargo workspace.
- Preserved only the two crate shells, the workspace manifests, minimal architecture/changelog docs, and the dependency link from `morbid-app` to `monarch-engine`.
- Replaced `monarch-engine` with a minimal no-op plugin skeleton and reduced `morbid-app` to a minimal Bevy app that depends on that plugin.
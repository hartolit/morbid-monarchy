# Agentic Changelog

- Improved chunk loading architecture by establishing the `chunks` table during `morbid-app` startup, moving authoritative procedural chunk generation into `monarch-engine` via `ChunkData::generate`, delegating cache-miss fallback in `morbid-app` to that engine API, and ensuring `monarch-engine` stores the normalized loaded chunk state after fast-forward handling.

# Architectural Blueprint: The Modular Philosophy

**CORE IDEOLOGY: DEFEATING THE MONOLITH**
This project strictly rejects monolithic structures. We do not build massive, tightly coupled "engine" or "core" crates where logic is tangled. We operate on a highly modular, decoupled architecture. Every distinct feature, system, or domain must exist as its own independent crate within a Cargo workspace.

**1. CRATE ONTOLOGY**
* **Feature Crates (The Building Blocks):** Isolated, heavily confined modules (e.g., `os-vga`, `game-water`). These must NEVER depend upward on the main engine or horizontally on unrelated feature crates. Design every crate as if it will be published to `crates.io` and used by an entirely different project tomorrow.
* **Engine/Core Crates (The Glue):** These crates do NOT implement core features. Instead, they consume Feature Crates and provide the orchestration, ECS integration, or state management to link them together cleanly.
* **App/Runner Crates (The Boundary):** Thin execution vectors (e.g., `kernel-runner`, `server-cli`). They handle initialization, config loading, and environment I/O, then pass control to the Engine.

**2. API BOUNDARIES & COUPLING LAWS**
* **Explicit Public APIs:** A crate's internals must be heavily encapsulated. Internal logic, state, and helper functions must remain strictly private. Expose only what is strictly necessary through a stable, well-documented API.
* **Dependency Injection:** Crates should not assume the existence of a global state. Pass necessary contexts, traits, or data down into the crate via its API.
* **Acyclic Dependencies:** Crates must form a clean, directed acyclic graph. Circular dependencies or "God objects" that weave crates back together are architectural failures.

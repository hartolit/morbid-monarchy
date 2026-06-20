# Architectural Manifest: Modular Systems and Low-Abstraction Design

**PURPOSE:** To establish the structural and physical constraints of the Rust workspace. This document dictates directory structure, memory invariants, execution boundaries, and production hygiene. Monolithic design is fundamentally rejected in favor of strict modular isolation to ensure predictable execution, high performance, and maintainable domain boundaries.

## 1. WORKSPACE STRUCTURE (THE MODULAR BLUEPRINT)
The project operates as a strictly partitioned Rust Cargo Workspace. Directories are separated by explicit domain responsibilities. Monolithic coupling is forbidden.

* **`crates/*-lib` (Domain Modules):** Distinct, self-contained mathematical or logical domains (e.g., `crates/spatial-lib`, `crates/cellular-automata`, `crates/input-lib`). These modules must be entirely agnostic of the wider system. They contain pure logic, state transitions, and algorithms. Zero environment-specific I/O or side-effects are permitted here. 
* **`crates/*-engine` (Core Orchestration):** The unyielding center that integrates the domain modules (`*-lib`) into a unified state machine. It orchestrates the flow of data between cellular, spatial, and physics domains but still remains completely isolated from system I/O, file paths, and network protocols.
* **`crates/*-app` (Execution Vectors):** The operational binaries (e.g., `-client`, `-cli`, `-server`). These crates are structurally thin consumers of the engine. They own the messy reality of startup routines, configuration loading, logging orchestration, and boundary I/O. They bridge the core truth to the external environment.
* **`crates/*-shared` / `crates/*-protocol`:** Instantiated strictly when multiple crates require shared vocabulary or data structures. Zero business logic is allowed to migrate here.

## 2. THE PHYSICS OF RUST (MEMORY AND HARDWARE MECHANICS)
* **Cache Line Alignment & Data Locality:** Structures processing high-frequency data (such as grids in `cellular-automata` or spatial partitioning trees in `spatial-lib`) must account for CPU cache line mechanics (64-byte blocks). Memory allocation must prioritize contiguous block arrangements (e.g., utilizing flat `Vec` or Struct-of-Arrays (SoA) patterns) to prevent pointer indirection, heap fragmentation, and L1/L2 cache evictions.
* **Zero-Copy Data Flow:** High-frequency data transfers between modules must execute via borrowing (`&[T]`, `&mut [T]`) or smart pointer handoffs. Passing large data structures by value in execution loops is forbidden. 
* **Borrowing vs. Cloning:** Utilize idiomatic error handling and data flow. Cloning data solely to bypass the borrow checker is strictly forbidden. Data flow must be explicitly designed around clear ownership constraints.

## 3. SYSTEM STATE AND CONFIGURATION
* **Strict Domain Consolidation:** Logic must reside exclusively in its designated crate. Leaking runtime I/O, file reading, or network requests into the `*-engine` or `*-lib` crates is a severe architectural breach.
* **Platform-Aware Configuration:** Hardcoded configuration values (magic numbers) are forbidden. Native targets must load tunable parameters from typed configuration (e.g., TOML). Compile-time defaults must be centralized in Rust types via the `Default` trait or dedicated configuration modules.

## 4. PRODUCTION HYGIENE AND LINGUISTIC INTEGRITY
* **Absolute Completeness:** There is no "prototype phase" allowed in committed code. Faking execution using dummy data (e.g., initializing an array of zeros to bypass actual computation) or simulated state is strictly banned. Every code block, algorithm, and module output must be mathematically and logically complete, compile-ready, and production-grade.
* **Ruthless Deletion:** Dead code, unused flags, TODOs, and commented-out placeholder logic must be eradicated. Rely on deterministic static analysis (`clippy`, `rustc` warnings). If a function or module is unused, delete it.
* **Rigorous Error Handling:** Use idiomatic error handling (`Result`/`?`) at all boundaries. Avoid `unwrap` and `expect` outside of testing environments unless enforcing an impossible invariant that has been mathematically or structurally guaranteed prior to execution.
* **Documentation Constraints:** Comments inside source code (`.rs` files) must be standard, highly rigorous engineering explanations of *how* and *why* an algorithm or memory layout operates. No philosophical, aesthetic, or conversational language is permitted inside the codebase. Variables must possess descriptive, undeniable meaning. Do not compress variable names into cryptic abbreviations.

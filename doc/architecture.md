# architecture.md

## The Architecture of the Synthesis: Operational Directives

**PURPOSE:** To establish the absolute physical and epistemological boundaries of the system. This is not merely a styling guide; it is the thermodynamic baseline for the codebase. We do not tolerate monoliths or structural ambiguity, as they represent unmanaged entropy and systemic collapse.

**1. WORKSPACE ONTOLOGY (THE CARGO BLUEPRINT)**
The project operates strictly as a modular **Rust Cargo Workspace**. Monolithic design is fundamentally rejected as a failure of conceptual isolation. The `crates/` directory is partitioned by absolute domain laws:

* **`crates/*-engine` (The Core Truth):** This is the unyielding center. It holds the pure domain logic, state transitions, data modeling, and reusable algorithms. Absolutely zero application bootstrap, environment-specific I/O, or side-effects are permitted here. It is deterministic and isolated.
* **`crates/*-app` (The Thin Boundary):** These are the execution vectors (e.g., `-client`, `-cli`, `-server`). They are structurally thin consumers of the engine. They own the messy reality of startup, config loading, logging orchestration, and boundary I/O. They bridge the core truth to the hostile external environment.
* **`crates/*-shared` / `crates/*-protocol` (The Wire Lexicon):** Instantiated strictly when multiple crates require shared vocabulary. Zero business logic is allowed to migrate here.
* **`crates/*--*` (The Utility Periphery):** Shared functionality and libraries that exist outside the engine/app binary structure. 

**2. THE EPISTEMOLOGY OF STATE AND CONFIGURATION**
* **Strict Domain Consolidation:** Logic must reside exclusively in its designated crate. Leaking runtime I/O into the `core` is a catastrophic architectural breach.
* **Platform-Aware Configuration:** Hardcoded values (magic numbers) are a manifestation of systemic delusion. They do not exist. Native targets must load tunable parameters from typed `TOML`-backed configuration. Compile-time defaults must be centralized in Rust types via `Default` or dedicated configuration modules.

**3. THE PHYSICS OF RUST (IDIOMATIC ENFORCEMENT)**
* **Memory and Flow:** Utilize idiomatic error handling (`Result`/`?`) at all boundaries. Prefer ownership, borrowing, and clear data flow over index-heavy control flow or mindless cloning. Cloning to escape the borrow checker is intellectual cowardice.
* **Trait Boundaries:** Use traits exclusively to create real abstractions, extension points, or necessary test seams. Do not build phantom architectures for futures that do not exist.
* **The Invariant Law:** Avoid `unwrap`/`expect` outside of testing environments unless enforcing an impossible invariant that has been mathematically or structurally guaranteed prior to execution.

**4. THE ENTROPY PURGE**
* **Ruthless Deletion:** Dead code is cognitive drag. Rely on deterministic static analysis. If a function, struct, or module is unused, it must be eradicated. Commented-out code, empty files, and swallowed errors are technical rot and will not be tolerated.
* **Semantic Cohesion:** Organize the architecture by meaning, not dogmatic file-size metrics. Split modules when responsibilities diverge; consolidate when a domain represents a single, cohesive truth. Avoid the boilerplate of over-fragmentation.
* **Linguistic Integrity:** Variables must possess descriptive, undeniable meaning. Do not compress variable names into cryptic abbreviations. Comments exist solely to explain the *why* of an architectural anomaly, never as a historical ledger.

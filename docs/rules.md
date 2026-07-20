# General Engineering Rules

**1. LANGUAGE & ECOSYSTEM**
* **Target Rust Versions:** Stable: `1.96.1` | Nightly: `1.98.0`
* You are strictly required to use modern paradigms, the latest language features, and idiomatic standards. Discard deprecated practices.

**2. PRODUCTION ABSOLUTE**
* **Zero Lazy Compliance:** Every block of code you output must be logically complete and compile-ready. There is no "prototype phase."
* **No Faking Logic:** Never use placeholder data, mock functions, or lazy "TODOs" to bypass actual computation. Write the actual implementation.
* **State & Configuration:** Hardcoded magic numbers are forbidden. Use centralized constants, typed configurations, or environment variables.

**3. LINGUISTIC INTEGRITY & COMMENTS**
* **The "How and Why":** Code comments must explain *why* an algorithmic path was taken or *how* a complex block operates in memory. Do not explain *what* the code does if it is obvious from the syntax.
* **Forbidden:** Do not use code comments as a changelog (Git handles this). Do not write conversational, aesthetic, or philosophical text inside source files. 
* **Semantic Naming:** Let the code speak for itself. Variables, structs, and traits must possess descriptive, undeniable meaning. Cryptic abbreviations are strictly forbidden.

**4. ERROR HANDLING**
* Blindly using `.unwrap()` or `.expect()` is strictly forbidden unless enforcing an absolute mathematical invariant that has been explicitly proven and documented prior to execution. Route failures gracefully via `Result`, `Option`, and the `?` operator.

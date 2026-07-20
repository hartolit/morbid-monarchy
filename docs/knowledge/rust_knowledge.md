# Rust Performance & Optimization Priorities

When writing Rust code for high-performance systems or bare-metal environments, hardware sympathy is paramount. However, it must be balanced against systemic stability and maintainability. Let the following concepts guide your code structure, strictly separated into "Hot Path" and "Cold Path" methodologies:

**1. ARCHITECTURE & MEMORY (THE HOT PATH)**
* **Data-Oriented Design (DoD) via ECS:** Structure data by how it is accessed by the CPU. Prefer Structs of Arrays (SoA) over Arrays of Structs (AoS) to maximize cache line density. **Footgun Guard:** Do not manually manage complex SoA structures; this causes ergonomic nightmares. Rely on an Entity-Component-System (ECS) to manage data contiguity automatically.
* **Cache Alignment & Prefetching:** In high-frequency loops (e.g., simulation ticks, rendering), process flat, contiguous memory arrays. Use zip iterators to lock the hardware prefetcher into a predictable linear stride.
* **Bulk Memory Operations:** When resetting buffers or wiping state, use `slice::copy_from_slice()` or bulk mutations. Standard library slice methods compile down to highly optimized, auto-vectorized `memcpy` or SIMD instructions.
* **Zero-Allocation Hot Paths:** Inside the core simulation, hot loops, or interrupt handlers, heap allocations (`Box`, `Vec`, `Arc`) are strictly forbidden. Rely on pre-allocated buffers, stack allocation, and slices. 
* **Memory-Mapped I/O (MMIO):** When interacting directly with hardware registers (e.g., VGA buffers, serial ports), always use `core::ptr::read_volatile` and `write_volatile`. The compiler's optimizer will ruthlessly delete standard memory reads/writes if it doesn't see a software-level side effect.

**2. CONCURRENCY & ATOMICS (CRITICAL SAFETY)**
* **The Atomics Footgun:** Never use `Ordering::Relaxed` as a default. `Relaxed` provides *no* synchronization guarantees across threads.
    * Use **`Ordering::SeqCst`** or **`Acquire/Release`** when an atomic variable acts as a lock or signals that *other* memory is safe to read (e.g., cross-thread communication).
    * Use **`Ordering::Relaxed`** *only* for independent counters, isolated flags, or bulk buffer wiping where memory ordering between threads is mathematically irrelevant.
* **Locks:** Favor lock-free architectures. When `spin::Mutex` is required, hold it for the absolute minimum CPU cycles. Never execute heavy logic inside a lock.

**3. COMPILER HINTS & PROFILING**
* **Trust LLVM First:** Rust's iterators are highly optimized. Write idiomatic Iterator chains first. Only drop down to raw contiguous slice loops if a profiler explicitly tells you a specific loop failed to auto-vectorize and is causing a bottleneck.
* **Instruction Cache (I-Cache) & Strategic Inlining:** * Do not blindly spam `#[inline(always)]` across the codebase; this bloats the I-Cache and causes costly evictions, slowing down the program.
    * **DO use `#[inline(always)]`** strategically for leaf functions, state accessors, mathematical derivations, and helper logic that are called repeatedly inside hot, high-frequency loops. The goal is to eliminate function call overhead and allow the compiler to optimize across the function boundary.
    * Use `#[cold]` for error handling, bounds-check failures, and panics to explicitly push them out of the hot execution path.
* **Branch Prediction Hinting:** Use `core::hint::unlikely` only in critical loops where branch mispredictions cause measurable pipeline stalls. For general application logic, rely on Profile-Guided Optimization (PGO) at the build level rather than manual hinting.
* **Compile-Time Execution (`const fn`):** Constructors and mathematical derivations should be marked `const fn` to offload runtime CPU cycles into static binary `.rodata`.

**4. ERGONOMICS (THE COLD PATH)**
* **Initialization & Setup:** During initial boot, config loading, or subsystem initialization, prioritize human readability and idiomatic Rust. It is acceptable to use `.clone()` or standard allocations here if it saves hours of architectural refactoring. Extreme optimization is reserved strictly for continuous runtime loops.

**5. UNSAFE BOUNDARIES & SOUNDNESS PROOFS**
* **Micro-Scoping & Containment:** Bare-metal development fundamentally relies on `unsafe` code to interact with hardware. However, `unsafe` blocks must be kept infinitesimally small and never leak into business logic or high-level engine crates. Wrap hardware interactions strictly behind safe, zero-cost abstractions (e.g., our `Uart16550Tty` implementation). 
* **Mandatory `// SAFETY:` Docstrings:** Every `unsafe fn` or `unsafe {}` block must be immediately preceded by a `// SAFETY:` comment explicitly proving why the operation cannot violate Rust's aliasing or memory guarantees. "Trust me" is not a proof. You must cite hardware manuals or environmental invariants (e.g., "The bootloader guarantees this physical address is mapped and valid").
* **Volatile vs. Atomic:** Never confuse the two. Use `core::ptr::read_volatile` and `write_volatile` *strictly* for Memory-Mapped I/O (MMIO) where side-effects matter to hardware. Use Atomics *strictly* for thread/interrupt synchronization. Volatile does not guarantee atomicity; Atomics do not guarantee hardware side-effects.

**6. INLINE & NAKED ASSEMBLY**
* **Explicit Clobbering in `asm!`:** When dropping into inline assembly for CPU identification or port I/O, you must explicitly declare every modified register using `out` or `inout`. Failure to report clobbered registers to LLVM will result in silent, catastrophic state corruption during optimization passes.
* **Optimization Flags:** Always tag `asm!` blocks with the most restrictive options applicable (`options(nomem)`, `options(nostack)`, `options(preserves_flags)`). This allows LLVM to optimize the surrounding Rust code aggressively.
* **Context Switches & Interrupts (`naked_asm!`):** When writing raw interrupt handlers or context switchers where the compiler's function prologue/epilogue would corrupt the stack frame, you must use the `#[unsafe(naked)]` attribute combined with the `naked_asm!` macro. Do not attempt to manually bypass prologues in standard functions.

**7. TYPE-STATE PROGRAMMING & ZERO-COST DISPATCH**
* **Zero-Cost State Machines:** Use Rust's type system (Generics, Zero-Sized Types, and `PhantomData`) to encode hardware state transitions. For example, a driver should transition from `Vga<Uninitialized>` to `Vga<Ready>` at compile time. Functions to draw pixels should only be implemented on `Vga<Ready>`.
* **Eliminate Runtime Checks:** By forcing hardware state dependencies into the type system, we eliminate the need for runtime `if is_ready { ... }` checks or `Option::unwrap()` calls in high-frequency hardware loops. Invalid hardware transitions simply fail to compile.
* **Ban Dynamic Dispatch (`dyn Trait`):** In the hot path and core kernel loops, `dyn Trait` is forbidden. The indirect branching overhead of vtables destroys branch prediction and prevents inlining. Force monomorphization via generics (`impl Trait` or `<T: Trait>`).

**8. KERNEL-SPECIFIC CPU STATE (FPU & SIMD)**
* **The Context Switch Cost:** By default, do not use floating-point mathematics (`f32`/`f64`) or explicitly trigger SIMD operations in the core kernel. When hardware interrupts fire, the CPU state must be saved. Saving extended register states (XMM, YMM, ZMM for AVX/SSE) costs hundreds of cycles. Rely on integer arithmetic. If vectorization is absolutely required, the FPU/SIMD state must be meticulously managed during context switches using `fxsave`/`fxrstor`.

**9. ERROR PROPAGATION IN NO_STD**
* **Enum-Driven Failures:** Without `core::error::Error` and dynamic allocations, all subsystem failures must be codified as dense, statically defined `enum` types.
* **Avoid String Errors:** Never use `&str` or `String` for error variants. Use explicitly named variants (e.g., `VgaBufferFull`, `InvalidOpcode`) to allow programmatic recovery without string parsing.

**10. CALLING CONVENTIONS & STRUCT PACKING (ABI AWARENESS)**
* **Architecture-Specific ABIs (Register vs. Stack):** Understand your target's ABI (Application Binary Interface) to avoid hidden memory overhead in function calls. Arguments are passed in fast CPU registers up to a hard limit before "spilling" into slower stack memory.
    * **x86_64 (System V ABI):** The first 6 integer arguments go into registers (`rdi`, `rsi`, `rdx`, `rcx`, `r8`, `r9`). Return values go in `rax` (and `rdx` for overflow).
    * **ARM64 (AAPCS64):** The first 8 integer arguments go into registers (`x0` through `x7`). Return values go in `x0` and `x1`. If returning a large struct by value, `x8` acts as an indirect pointer to memory allocated by the caller.
    * **Hot Path Rule:** Keep high-frequency function signatures small to ensure arguments remain entirely within CPU registers.
* **The 16-Byte Struct Limit:** When passing structs by value, the compiler evaluates their total size to determine placement.
    * **<= 16 Bytes:** Structs 16 bytes or smaller are typically split and packed into 1 to 2 available registers.
    * **> 16 Bytes:** Structs larger than 16 bytes (e.g., a 24-byte struct of six `i32`s) bypass registers entirely and are pushed to the stack. On ARM64, structs larger than 16 bytes are generally passed by reference behind the scenes.
* **Hot Path Optimization Strategy:**
    * **Shrink Data Types:** Use smaller primitives (e.g., `i16` instead of `i32` for coordinates) to keep your hot-path structs strictly under the 16-byte limit, ensuring they execute entirely within CPU registers.
    * **By-Reference over By-Value for Bulky Data:** If a struct exceeds architecture register packing limits, pass it by reference (`&T` or `&mut T`). A reference is always a standard pointer size (8 bytes on 64-bit systems), ensuring it uses exactly one CPU register regardless of the underlying data size.
* **Stable ABIs & Interrupt Context:** The default "Rust" calling convention is internal and unstable.
    * **Hardware Boundaries:** When writing hardware-facing code—such as CPU exception handlers, interrupt service routines (ISRs), or FFI bounds—you must explicitly declare the ABI. Use `extern "C"`, `extern "x86-interrupt"`, or `extern "aapcs"` to guarantee the compiler interacts with the hardware's expected register state accurately.
    * **Context Preservation (Clobbering):** When writing exception handlers, you are actively interrupting another function's ABI flow. The hardware will not save standard registers for you. You must manually preserve (push) and restore (pop) all volatile registers dictated by the ABI to prevent catastrophic state corruption in the interrupted function.

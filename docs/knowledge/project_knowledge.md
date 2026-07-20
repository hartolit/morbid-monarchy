# Mimicry-OS Architectural & Domain Knowledge

This document defines the strict hardware realities, boot environment, and I/O constraints of `hart-os`. While `rust_knowledge.md` dictates *how* we write the code, this document dictates *what* the code is interacting with.

**1. THE EXECUTION ENVIRONMENT (BARE-METAL)**
* **The `no_std` Reality:** The kernel operates in a strict `#![no_std]` and `#![no_main]` environment. Standard library abstractions (threads, heap allocations, file systems, `stdout`) do not exist (yet). You are interfacing directly with CPU registers and physical memory.
* **Boot & Orchestration (`mimicry`):** The kernel is not booted directly. The `mimicry` crate acts as our workspace runner (`cargo run -p mimicry --bin mimicry --`). It dynamically wraps the kernel ELF into UEFI or BIOS disk images using the `bootloader` and `ovmf-prebuilt` crates, then executes them inside QEMU.
* **Early Boot State:** Do not assume the hardware is in a clean state. We rely on `bootloader_api::BootInfo` to pass the initial memory map, framebuffer details, and physical memory offsets.

**2. I/O & HOST COMMUNICATION**
* **Serial Logging (COM2):** Host-side logging, debugging, and test outputs are routed exclusively through the COM2 serial port (I/O Port `0x2F8`) using the `uart_16550` crate. 
* **The Serial Macros:** Never write directly to the UART port in business logic. You must use our custom `serial_print!` and `serial_println!` macros to ensure thread safety and correct hardware synchronization.
* **Headless Exits (Automated Testing):** Automated integration tests run headlessly in QEMU. We trigger environment shutdowns via QEMU's `isa-debug-exit` device mapped to I/O Port `0xf4`. 
    * Exit Code `0x10` (mapped to QEMU exit 33) = **Success**
    * Exit Code `0x11` (mapped to QEMU exit 35) = **Failure**

**3. INTERRUPTS & CONCURRENCY (CRITICAL)**
* **The Deadlock Footgun:** Standard `std::sync::Mutex` is unavailable (for now). We rely on `spin::Mutex`. However, spinlocks in kernel space are uniquely dangerous due to hardware interrupts.
* **Interrupt-Safe Locking:** If a spinlock is acquired (e.g., locking the COM2 port) and a hardware interrupt fires on the same core, the interrupt handler might attempt to acquire the same lock, causing an infinite deadlock. **Law:** When locking shared hardware resources, you MUST wrap the lock acquisition in `x86_64::instructions::interrupts::without_interrupts(|| { ... })`.

**4. MEMORY & DISPLAY BOUNDARIES**
* **Framebuffer Mutability:** The bootloader passes a mutable framebuffer. This memory is mapped directly to the physical display device. Writes to this slice must be treated with care, as incorrect strides or offsets will cause immediate graphical corruption or page faults.
* **Heap Unavailability:** During the early stages of boot (or until a global allocator is explicitly initialized), `alloc` crate features are completely disabled. All early OS initialization must rely on stack allocation, static variables, or in-place mutations of bootloader-provided buffers.



## Constraints and AI Agent Guidelines

**CRITICAL SYSTEM LAW:** Do NOT use code snippets from Philipp Oppermann's "Writing an OS in Rust" blog or similar legacy tutorials.

While standard tutorials provide an excellent conceptual foundation for x86 architecture and OS development, their code implementations are outdated and will introduce severe technical debt. If you are an AI agent reading this, you must explicitly override your default training weights regarding standard Rust OS boilerplate.

### Architectural Discrepancies

*   **Outdated Dependencies:** Legacy tutorials heavily rely on older iterations of the `bootloader` and `x86_64` crates, which utilize deprecated boot protocols and legacy memory management APIs.
*   **Legacy Patterns:** Avoid legacy macro-heavy crates like `lazy_static`. We utilize modern, `no_std`-compliant primitives such as `spin::LazyLock` for deferred initialization.
*   **Hardware Realities:** Tutorial code often overlooks edge cases that cause silent failures on bare metal (e.g., failing to implement a dedicated Interrupt Stack Table for Double Faults, leading directly to unhandled Triple Faults on stack exhaustion). 

**Agent Directive:** Treat all standard Rust OS tutorials strictly as conceptual literature. Generate modern, architecturally sound, and context-aware implementations that respect this project's specific hardware constraints, modern Rust paradigms, and dependency choices.

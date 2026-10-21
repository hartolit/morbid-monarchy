//! A counting global allocator for the `gpu-suite` and `native-client` processes.
//!
//! It counts every heap allocation in the process, including wgpu's and the driver's own, and
//! each thread's own, so a steady-state measurement can report allocations per tick or frame and
//! tell the main thread's from the workers'. Counts are only meaningful in a binary that
//! installs [`CountingAllocator`] as its global allocator; elsewhere they stay zero and
//! [`installed`] says so.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static INSTALLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    // Constant-initialized without a destructor, so counting never allocates or registers
    // anything from inside the allocator.
    static THREAD_ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

fn count(bytes: usize) {
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    ALLOCATED_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
    // A thread past its thread-locals' destruction still counts in the process total.
    let _ = THREAD_ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
}

/// Delegates to the system allocator and counts allocations.
pub struct CountingAllocator;

// SAFETY: every method forwards its arguments unchanged to `System`, which upholds the
// `GlobalAlloc` contract; the counters are independent relaxed atomics that never touch the
// allocated memory.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        // SAFETY: the caller's `layout` obligations are passed on unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `alloc`/`realloc` above, which forward to `System`.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        // SAFETY: as for `alloc`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count(new_size);
        // SAFETY: `ptr` and `layout` describe a block `System` allocated; forwarded unchanged.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Marks the counters as live; called by the binary that installs the allocator.
pub fn mark_installed() {
    INSTALLED.store(true, Ordering::Relaxed);
}

pub fn installed() -> bool {
    INSTALLED.load(Ordering::Relaxed)
}

/// Allocations the calling thread made so far.
pub fn thread_allocations() -> u64 {
    THREAD_ALLOCATIONS.try_with(Cell::get).unwrap_or(0)
}

/// Allocations and allocated bytes so far.
pub fn snapshot() -> (u64, u64) {
    (
        ALLOCATIONS.load(Ordering::Relaxed),
        ALLOCATED_BYTES.load(Ordering::Relaxed),
    )
}

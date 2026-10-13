//! A counting global allocator for the `gpu-suite` process.
//!
//! It counts every heap allocation in the process, including wgpu's and the driver's own, so a
//! steady-state measurement can report allocations per tick. Counts are only meaningful in the
//! binary that installs [`CountingAllocator`] as its global allocator; elsewhere they stay zero
//! and [`installed`] says so.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Delegates to the system allocator and counts allocations.
pub struct CountingAllocator;

// SAFETY: every method forwards its arguments unchanged to `System`, which upholds the
// `GlobalAlloc` contract; the counters are independent relaxed atomics that never touch the
// allocated memory.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: the caller's `layout` obligations are passed on unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `alloc`/`realloc` above, which forward to `System`.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: as for `alloc`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
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

/// Allocations and allocated bytes so far.
pub fn snapshot() -> (u64, u64) {
    (
        ALLOCATIONS.load(Ordering::Relaxed),
        ALLOCATED_BYTES.load(Ordering::Relaxed),
    )
}

// Copyright 2026 Goldman Sachs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Custom allocator wrapper for tracking peak memory during benchmarks.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A snapshot of memory allocation statistics.
#[derive(Debug, Clone, Copy, Default)]
pub struct AllocStats {
    /// The peak number of allocated bytes since the last reset.
    pub peak_bytes: usize,
    /// The total number of allocations performed since the last reset.
    pub alloc_count: usize,
}

/// Global tracking allocator.
pub struct TrackingAllocator;

static CURRENT_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();
        let prev_current = CURRENT_BYTES.fetch_add(size, Ordering::SeqCst);
        let new_current = prev_current + size;

        // Update peak if necessary
        let mut prev_peak = PEAK_BYTES.load(Ordering::SeqCst);
        while new_current > prev_peak {
            match PEAK_BYTES.compare_exchange_weak(
                prev_peak,
                new_current,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(new_prev) => prev_peak = new_prev,
            }
        }

        ALLOC_COUNT.fetch_add(1, Ordering::SeqCst);

        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = layout.size();
        CURRENT_BYTES.fetch_sub(size, Ordering::SeqCst);
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// Resets the allocation statistics to zero.
/// This should be called immediately before entering the benchmark measurement loop.
pub fn reset() {
    // Note: We don't reset CURRENT_BYTES because we want to track the actual heap usage delta,
    // and resetting it to 0 while existing objects are allocated would cause underflow on dealloc.
    // Instead, we set PEAK_BYTES to match CURRENT_BYTES as the new high-water mark starting point,
    // and reset the allocation count.

    let current = CURRENT_BYTES.load(Ordering::SeqCst);
    PEAK_BYTES.store(current, Ordering::SeqCst);
    ALLOC_COUNT.store(0, Ordering::SeqCst);
}

/// Returns a snapshot of the allocation statistics since the last reset.
#[must_use]
pub fn snapshot() -> AllocStats {
    let peak = PEAK_BYTES.load(Ordering::SeqCst);

    // The peak delta from the time of reset
    // peak is at least current (or whatever it was at reset)
    // If no allocs happened, peak == current_at_reset.

    // Instead of doing complicated math, we just take the raw peak. The user should compute
    // delta if they want, but PEAK_BYTES.load() - CURRENT_BYTES.load_at_reset() is what we usually want.
    // However, if we just want the peak of this run, we can also just expose current peak.

    // Let's provide a simpler definition of "peak bytes" as the maximum size allocated since reset,
    // rather than total footprint. But since deallocs happen, maybe it's simpler.

    AllocStats {
        peak_bytes: peak,
        alloc_count: ALLOC_COUNT.load(Ordering::SeqCst),
    }
}

/// Calculates the max memory usage delta.
pub fn peak_delta(baseline_current: usize) -> usize {
    let peak = PEAK_BYTES.load(Ordering::SeqCst);
    peak.saturating_sub(baseline_current)
}

/// Returns the current number of allocated bytes.
pub fn current_bytes() -> usize {
    CURRENT_BYTES.load(Ordering::SeqCst)
}

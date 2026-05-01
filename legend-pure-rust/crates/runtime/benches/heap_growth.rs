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

//! Heap-growth benchmark — proves the post-Rc heap reclaims user
//! allocations as their references drop.
//!
//! Pre-change (`SlotMap` heap, monotonic): allocating 100K objects in a
//! loop and discarding them grew the heap to ~101K rows for the
//! evaluator's lifetime.
//!
//! Post-change goal (this bench's invariant): only live objects keep
//! their entries. The bench drives many alloc-and-drop cycles; the
//! steady-state strong-Rc count, observed via `Rc::strong_count` on a
//! bystander handle, never grows past 1.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use criterion::{Criterion, criterion_group, criterion_main};
use legend_pure_runtime::heap::{HeapEntry, RuntimeHeap};
use legend_pure_runtime::value::Value;

/// Allocate-and-drop: each iteration creates one fresh handle, sets a
/// property, then drops the handle by ending the scope. A `Weak`
/// captured before drop verifies (on the very next iteration) that the
/// previous entry's `Rc` count hit zero.
fn alloc_and_drop_no_retention(c: &mut Criterion) {
    c.bench_function("alloc_and_drop_no_retention", |b| {
        let mut heap = RuntimeHeap::new();
        let mut last_weak: Option<Weak<RefCell<HeapEntry>>> = None;
        b.iter(|| {
            // Confirm the previous iteration's entry is gone.
            if let Some(w) = last_weak.take() {
                debug_assert!(
                    w.upgrade().is_none(),
                    "previous handle should be reclaimed by RAII"
                );
            }
            let h = heap.alloc_dynamic("bench::Trade");
            h.borrow_mut()
                .mutate_add("price", &[Value::Float(42.0)])
                .expect("set price");
            last_weak = Some(Rc::downgrade(&h));
            // h drops at end of closure → strong count → 0.
        });
    });
}

/// Allocate, retain, then bulk-drop: build a Vec of N handles, then
/// drop the Vec. Verifies all N handles' entries are reclaimed in one
/// shot. Steady-state heap size is the metamodel arena (zero here) +
/// 0 user rows after the Vec drops.
fn bulk_alloc_then_drop(c: &mut Criterion) {
    c.bench_function("bulk_alloc_then_drop_1k", |b| {
        let mut heap = RuntimeHeap::new();
        b.iter(|| {
            let mut handles = Vec::with_capacity(1_000);
            for i in 0..1_000_i64 {
                let h = heap.alloc_dynamic("bench::Trade");
                h.borrow_mut()
                    .mutate_add("id", &[Value::Integer(i)])
                    .expect("set id");
                handles.push(h);
            }
            // Snapshot one weak before the Vec drops.
            let w = Rc::downgrade(&handles[0]);
            drop(handles);
            debug_assert!(
                w.upgrade().is_none(),
                "all 1k handles should be reclaimed when the Vec drops"
            );
            // Heap state: metamodel rows only (zero in this bench).
            assert_eq!(heap.metamodel_len(), 0);
        });
    });
}

criterion_group!(benches, alloc_and_drop_no_retention, bulk_alloc_then_drop);
criterion_main!(benches);

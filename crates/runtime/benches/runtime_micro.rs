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

//! Micro-benchmarks for runtime core operations.
//!
//! Run: `cargo bench --bench runtime_micro`
//!
//! These benchmarks track the per-operation cost of the runtime's core
//! primitives. They serve as the Tier 1 regression gate — any PR that
//! regresses these by >20% should be investigated.

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use im_rc::{HashMap as PMap, Vector as PVector};
use rust_decimal::Decimal;
use smol_str::SmolStr;

use legend_pure_runtime::date::{PureDate, TimePrecision};
use legend_pure_runtime::heap::RuntimeHeap;
use legend_pure_runtime::value::{Value, ValueKey};

// ---------------------------------------------------------------------------
// Value Operations
// ---------------------------------------------------------------------------

fn bench_value_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("value_ops");

    group.bench_function("integer_create_match", |b| {
        b.iter(|| {
            let v = Value::Integer(black_box(42));
            match v {
                Value::Integer(i) => black_box(i),
                _ => unreachable!(),
            }
        });
    });

    group.bench_function("float_create_match", |b| {
        b.iter(|| {
            let v = Value::Float(black_box(3.14));
            match v {
                Value::Float(f) => black_box(f),
                _ => unreachable!(),
            }
        });
    });

    group.bench_function("string_clone_short", |b| {
        let v = Value::String(SmolStr::new("hello")); // inline SmolStr
        b.iter(|| black_box(v.clone()));
    });

    group.bench_function("string_clone_long", |b| {
        let v = Value::String(SmolStr::new("this is a longer string that exceeds 24 bytes"));
        b.iter(|| black_box(v.clone()));
    });

    group.bench_function("collection_clone_empty", |b| {
        let v = Value::Collection(PVector::new());
        b.iter(|| black_box(v.clone()));
    });

    group.bench_function("collection_clone_1000", |b| {
        let mut pv = PVector::new();
        for i in 0..1000 {
            pv.push_back(Value::Integer(i));
        }
        let v = Value::Collection(pv);
        b.iter(|| black_box(v.clone())); // O(1) clone due to structural sharing
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Property Access
// ---------------------------------------------------------------------------

fn bench_property_access(c: &mut Criterion) {
    let mut group = c.benchmark_group("property_access");

    // Dynamic object: HashMap lookup
    group.bench_function("dynamic_get", |b| {
        let mut heap = RuntimeHeap::new();
        let id = heap.alloc_dynamic("my::Trade");
        heap.mutate_add(id, "price", &[Value::Float(42.0)]).unwrap();
        heap.mutate_add(id, "ticker", &[Value::String("AAPL".into())])
            .unwrap();

        b.iter(|| black_box(heap.get_property(id, "price").unwrap()));
    });

    // Dynamic object: mutateAdd single value
    group.bench_function("dynamic_mutate_add", |b| {
        let mut heap = RuntimeHeap::new();
        let id = heap.alloc_dynamic("my::Account");

        b.iter(|| {
            heap.mutate_add(id, "values", &[Value::Integer(black_box(1))])
                .unwrap();
        });
    });

    // Object allocation
    group.bench_function("alloc_dynamic", |b| {
        let mut heap = RuntimeHeap::new();
        b.iter(|| {
            let id = heap.alloc_dynamic(black_box("my::Trade"));
            black_box(id);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Collection Operations (HAMT vs std HashMap)
// ---------------------------------------------------------------------------

fn bench_collections(c: &mut Criterion) {
    let mut group = c.benchmark_group("collections");

    // HAMT persistent put (what Rust runtime uses)
    for size in [100, 1_000, 10_000] {
        group.bench_function(format!("hamt_put_{size}"), |b| {
            b.iter(|| {
                let mut map = PMap::<ValueKey, Value>::new();
                for i in 0..size {
                    map.insert(ValueKey::Integer(i), Value::Integer(i));
                }
                black_box(map)
            });
        });
    }

    // std HashMap clone-per-put (simulates Java compiled fold+put)
    for size in [100, 1_000] {
        group.bench_function(format!("std_clone_put_{size}"), |b| {
            b.iter(|| {
                let mut map = std::collections::HashMap::<i64, Value>::new();
                for i in 0..size {
                    let mut new_map = map.clone();
                    new_map.insert(i, Value::Integer(i));
                    map = new_map;
                }
                black_box(map)
            });
        });
    }
    // NOTE: std_clone_put_10000 intentionally omitted — it takes seconds

    // PVector push_back (persistent append)
    for size in [100, 1_000, 10_000] {
        group.bench_function(format!("pvector_push_{size}"), |b| {
            b.iter(|| {
                let mut vec = PVector::<Value>::new();
                for i in 0..size {
                    vec.push_back(Value::Integer(i));
                }
                black_box(vec)
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Decimal & Date Operations
// ---------------------------------------------------------------------------

fn bench_decimal_date(c: &mut Criterion) {
    let mut group = c.benchmark_group("decimal_date");

    // Decimal arithmetic — the reason we use rust_decimal instead of SmolStr
    group.bench_function("decimal_add", |b| {
        let a = Decimal::new(1050, 2); // 10.50
        let bb = Decimal::new(325, 2); // 3.25
        b.iter(|| black_box(a + bb));
    });

    group.bench_function("decimal_multiply", |b| {
        let a = Decimal::new(1050, 2);
        let bb = Decimal::new(325, 2);
        b.iter(|| black_box(a * bb));
    });

    group.bench_function("decimal_create_value", |b| {
        b.iter(|| {
            let v = Value::Decimal(Decimal::new(black_box(4200), 2));
            black_box(v)
        });
    });

    // Date creation and arithmetic
    group.bench_function("date_create_strict", |b| {
        b.iter(|| {
            black_box(PureDate::strict_date(
                black_box(2024),
                black_box(3),
                black_box(15),
            ).unwrap())
        });
    });

    group.bench_function("date_add_days", |b| {
        let d = PureDate::strict_date(2024, 3, 15).unwrap();
        b.iter(|| black_box(d.add_days(black_box(10)).unwrap()));
    });

    group.bench_function("date_add_months", |b| {
        let d = PureDate::strict_date(2024, 3, 15).unwrap();
        b.iter(|| black_box(d.add_months(black_box(3)).unwrap()));
    });

    group.bench_function("datetime_create", |b| {
        b.iter(|| {
            black_box(
                PureDate::datetime(
                    black_box(2024),
                    black_box(3),
                    black_box(15),
                    black_box(10),
                    black_box(30),
                    black_box(0),
                    black_box(123_000_000),
                    TimePrecision::Subsecond(3),
                )
                .unwrap(),
            )
        });
    });

    group.bench_function("date_compare", |b| {
        let d1 = PureDate::strict_date(2024, 3, 15).unwrap();
        let d2 = PureDate::strict_date(2024, 6, 20).unwrap();
        b.iter(|| black_box(d1 < d2));
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    bench_value_ops,
    bench_property_access,
    bench_collections,
    bench_decimal_date,
);
criterion_main!(benches);

# Benchmarking Strategy

## Philosophy

Benchmark **early, continuously, comparatively**. Every architectural claim (HAMT is faster, struct access is faster, memoization helps) must have a number attached. No number = no claim.

---

## Three Tiers

### Tier 1: Micro-Benchmarks (Per-Operation)

**Tool**: [criterion.rs](https://github.com/bheisler/criterion.rs)  
**What**: Individual operations in isolation — the building blocks.  
**When**: Run with every PR. Regression gate in CI.

```toml
# Cargo.toml
[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }

[[bench]]
name = "runtime_micro"
harness = false
```

#### Benchmark Suite

```rust
// benches/runtime_micro.rs
use criterion::{criterion_group, criterion_main, Criterion, black_box};

fn value_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("value_ops");
    
    // Baseline: how fast is Value creation and matching?
    group.bench_function("integer_create_match", |b| {
        b.iter(|| {
            let v = Value::Integer(black_box(42));
            match v {
                Value::Integer(i) => black_box(i),
                _ => unreachable!(),
            }
        })
    });
    
    group.bench_function("string_clone", |b| {
        let s = Value::String(SmolStr::from("hello_world"));
        b.iter(|| black_box(s.clone()))
    });
    
    group.bench_function("collection_push_back", |b| {
        let vec = im_rc::Vector::<Value>::new();
        b.iter(|| {
            let mut v = vec.clone();
            v.push_back(Value::Integer(1));
            black_box(v)
        })
    });
    
    group.finish();
}

fn property_access(c: &mut Criterion) {
    let mut group = c.benchmark_group("property_access");
    
    // Dynamic: HashMap<SmolStr, PVector<Value>>
    group.bench_function("dynamic_hashmap", |b| {
        let mut heap = RuntimeHeap::new();
        let id = heap.alloc_dynamic("Trade");
        heap.mutate_add(id, "price", &[Value::Float(42.0)]).unwrap();
        
        b.iter(|| black_box(heap.get_property(id, "price")))
    });
    
    // Typed: TypedObject::get_property (match on name)
    group.bench_function("typed_match", |b| {
        let mut heap = RuntimeHeap::new();
        let trade = Trade { ticker: "AAPL".into(), price: 150.0, quantity: 100 };
        let id = heap.alloc_typed(trade);
        
        b.iter(|| black_box(heap.get_property(id, "price")))
    });
    
    // Struct: direct field access via downcast
    group.bench_function("struct_field", |b| {
        let mut heap = RuntimeHeap::new();
        let trade = Trade { ticker: "AAPL".into(), price: 150.0, quantity: 100 };
        let id = heap.alloc_typed(trade);
        
        b.iter(|| {
            let t: &Trade = heap.downcast_ref(id).unwrap();
            black_box(t.price)
        })
    });
    
    group.finish();
}

fn function_dispatch(c: &mut Criterion) {
    let mut group = c.benchmark_group("dispatch");
    
    group.bench_function("native_call", |b| {
        // Direct native function call (e.g., plus)
        b.iter(|| /* ... */)
    });
    
    group.bench_function("compiled_call", |b| {
        // CompiledFunction trait dispatch
        b.iter(|| /* ... */)
    });
    
    group.bench_function("interpreted_call", |b| {
        // Full expression tree walk
        b.iter(|| /* ... */)
    });
    
    group.bench_function("memo_hit", |b| {
        // Memoized function — cache hit path
        b.iter(|| /* ... */)
    });
    
    group.finish();
}

fn collection_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("collections");
    
    for size in [100, 1_000, 10_000, 100_000] {
        group.bench_with_input(
            BenchmarkId::new("hamt_put", size),
            &size,
            |b, &n| {
                b.iter(|| {
                    let mut map = im_rc::HashMap::new();
                    for i in 0..n {
                        map.insert(i, Value::Integer(i as i64));
                    }
                    black_box(map)
                })
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("std_hashmap_clone_put", size),
            &size,
            |b, &n| {
                b.iter(|| {
                    let mut map = std::collections::HashMap::new();
                    for i in 0..n {
                        // Simulate Java semantics: clone-on-each-put in a fold
                        let mut new_map = map.clone();
                        new_map.insert(i, Value::Integer(i as i64));
                        map = new_map;
                    }
                    black_box(map)
                })
            },
        );
    }
    
    group.finish();
}

criterion_group!(
    benches,
    value_operations,
    property_access,
    function_dispatch,
    collection_operations,
);
criterion_main!(benches);
```

**Expected output** (criterion generates HTML reports):

```
value_ops/integer_create_match   time: [1.2 ns 1.3 ns 1.4 ns]
value_ops/string_clone           time: [4.5 ns 4.7 ns 4.9 ns]
property_access/dynamic_hashmap  time: [28 ns 30 ns 32 ns]
property_access/typed_match      time: [8 ns 9 ns 10 ns]
property_access/struct_field     time: [1.1 ns 1.2 ns 1.3 ns]
collections/hamt_put/10000       time: [1.2 ms 1.3 ms 1.4 ms]
collections/std_clone_put/10000  time: [450 ms 480 ms 510 ms]  ← O(N²)
```

---

### Tier 2: Scenario Benchmarks (Real Pure Programs)

**Tool**: criterion.rs with setup/teardown  
**What**: Complete Pure programs that exercise specific architectural features.  
**When**: Run weekly or on significant changes.

Each scenario tests a specific claim from our architecture:

#### Scenario 1: Fold + Map.put (HAMT vs Clone)

```pure
// bench_fold_put.pure
function bench::foldPut(n: Integer[1]): Map<String, Integer>[1] {
    range(1, $n)->fold(
        {i, acc | $acc->put('key_' + toString($i % 1000), $i)},
        ^Map<String, Integer>()
    )
}
```

**Tests**: HAMT structural sharing. Should show O(N log N) vs O(N²).  
**Sizes**: n = 100, 1K, 10K, 100K  
**Expected**: Linear scaling with Rust HAMT; quadratic with Java HashMap clone.

#### Scenario 2: Object-Heavy Loop (Struct vs HashMap)

```pure
// bench_vwap.pure
Class bench::Trade {
    ticker: String[1];
    price: Float[1];
    quantity: Integer[1];
}

function bench::computeVWAP(trades: bench::Trade[*]): Float[1] {
    let totalNotional = $trades->map(t | $t.price * $t.quantity)->sum();
    let totalQuantity = $trades->map(t | $t.quantity)->toFloat()->sum();
    $totalNotional / $totalQuantity;
}
```

**Tests**: Property access speed. Dynamic vs struct.  
**Sizes**: 1K, 10K, 100K trades  
**Measure**: Total time AND property-access-only time via profiling.

#### Scenario 3: Relation GroupBy (Native Columnar)

```pure
// bench_relation.pure
function bench::groupBySum(r: Relation<(category: String, amount: Float)>[1]): 
    Relation<(category: String, total: Float)>[1] {
    $r->groupBy(~[category], ~[total : x | $x.amount : y | $y->sum()])
}
```

**Tests**: Native columnar engine performance.  
**Sizes**: 10K, 100K, 1M rows, varying group counts (10, 100, 10K).

#### Scenario 4: Recursive Function (Memoization)

```pure
// bench_fib.pure — pathological case for memo
function bench::fib(n: Integer[1]): Integer[1] {
    if($n <= 1, | $n, | bench::fib($n - 1) + bench::fib($n - 2))
}
```

**Tests**: Memoization effectiveness.  
**Without memo**: O(2^N). With memo: O(N).  
**Sizes**: n = 20, 30, 40

#### Scenario 5: Model Compilation (End-to-End)

```pure
// Load and compile an entire Pure project
// Measures: parse time, arena construction, index building
```

**Tests**: Cold start time. Parse + compile.  
**Input**: The Legend standard library (~500 Pure files).  
**Measure**: Wall clock from `load_sources()` to `model_ready()`.

---

### Tier 3: Parity Benchmarks (Rust vs Java)

**Tool**: [JMH](https://openjdk.org/projects/code-tools/jmh/) for Java, criterion for Rust  
**What**: Same Pure program, same inputs, both runtimes.  
**When**: Monthly or at milestones.

#### Setup

```
legend-benchmarks/
├── pure-sources/              ← shared Pure programs
│   ├── fold_put.pure
│   ├── vwap.pure
│   └── relation_groupby.pure
│
├── java-bench/                ← JMH benchmarks
│   ├── pom.xml
│   └── src/main/java/
│       └── PureBenchmark.java
│
├── rust-bench/                ← criterion benchmarks
│   ├── Cargo.toml
│   └── benches/
│       └── pure_bench.rs
│
└── compare.py                 ← compares JMH + criterion output
```

#### Java Side (JMH)

```java
@BenchmarkMode(Mode.AverageTime)
@OutputTimeUnit(TimeUnit.MICROSECONDS)
@State(Scope.Thread)
public class PureBenchmark {
    
    private PureRuntime runtime;
    private CoreInstance foldPutFn;
    
    @Setup
    public void setup() {
        // Load and compile Pure — include setup time separately
        runtime = new PureRuntimeBuilder(...)
            .build()
            .initialize();
        foldPutFn = runtime.getFunction("bench::foldPut_Integer_1__Map_1_");
    }
    
    @Benchmark
    public Object foldPut_10K_interpreted() {
        return interpretedExec.start(foldPutFn, 
            Lists.fixedSize.of(integerValue(10_000)));
    }
    
    @Benchmark
    public Object foldPut_10K_compiled() {
        return compiledExec.start(foldPutFn, 
            Lists.fixedSize.of(integerValue(10_000)));
    }
}
```

#### Comparison Script

```python
# compare.py — parses JMH JSON + criterion JSON, produces comparison table
import json

def compare(jmh_results, criterion_results):
    print(f"{'Benchmark':<35} {'Java Interp':>12} {'Java Compiled':>14} {'Rust Interp':>12} {'Ratio':>8}")
    print("─" * 85)
    for bench in BENCHMARKS:
        ji = jmh_results[f"{bench}_interpreted"]
        jc = jmh_results[f"{bench}_compiled"]
        ri = criterion_results[bench]
        ratio = f"{jc/ri:.1f}x"
        print(f"{bench:<35} {ji:>10.0f}μs {jc:>12.0f}μs {ri:>10.0f}μs {ratio:>8}")
```

**Expected output:**

```
Benchmark                           Java Interp  Java Compiled  Rust Interp    Ratio
─────────────────────────────────────────────────────────────────────────────────────
foldPut/10K                            2000μs          500μs          5μs    100x
vwap/100K_trades                        800μs          120μs         80μs     1.5x
relation_groupBy/100K                   300μs           50μs         30μs     1.7x
fib/30 (no memo)                      10000μs         2000μs       3000μs     0.7x
fib/30 (with memo)                     N/A             N/A            2μs      ∞
cold_start                           40000ms          40000ms       2500ms    16x
memory_peak                           1024MB          1024MB        100MB    10x
```

---

## CI Integration

### Per-PR Gate (Tier 1 only — fast)

```yaml
# .github/workflows/bench.yml
bench:
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - name: Run micro-benchmarks
      run: cargo bench --bench runtime_micro -- --output-format bencher | tee bench_output.txt
    
    - name: Check for regressions
      uses: benchmark-action/github-action-benchmark@v1
      with:
        tool: 'cargo'
        output-file-path: bench_output.txt
        alert-threshold: '120%'       # Fail if >20% regression
        fail-on-alert: true
        github-token: ${{ secrets.GITHUB_TOKEN }}
        comment-on-alert: true        # Post comment on PR
```

### Weekly Full Suite (Tiers 1+2)

```yaml
schedule:
  - cron: '0 6 * * 1'  # Every Monday 6am
steps:
  - run: cargo bench --bench runtime_micro --bench scenario_bench
  - name: Publish results
    run: python3 publish_results.py >> $GITHUB_STEP_SUMMARY
```

### Milestone Parity Check (Tier 3 — manual trigger)

```yaml
workflow_dispatch:  # Manual trigger
steps:
  - name: Run Rust benchmarks
    run: cargo bench --bench parity_bench -- --save-baseline rust
  - name: Run Java benchmarks
    run: cd java-bench && mvn exec:exec -Djmh.output=jmh_results.json
  - name: Compare
    run: python3 compare.py rust_results.json jmh_results.json
```

---

## Progress Dashboard

Track these **key metrics** over time:

```
┌──────────────────────────────────────────────────────┐
│  Legend Pure Runtime — Performance Dashboard          │
│                                                       │
│  Build: #247    Date: 2026-04-15    Commit: a1b2c3d  │
│                                                       │
│  ┌─ Core Operations ────────────────────────────────┐ │
│  │ Property access (dynamic):   31ns  (target: <50) │ │
│  │ Property access (struct):    1.2ns (target: <5)  │ │
│  │ Function dispatch:           22ns  (target: <30) │ │
│  │ Value::Integer create+match: 1.3ns (target: <5)  │ │
│  │ HAMT put (10K items):        1.3ms (target: <5)  │ │
│  └──────────────────────────────────────────────────┘ │
│                                                       │
│  ┌─ Scenarios ──────────────────────────────────────┐ │
│  │ foldPut/10K:          5μs   [vs Java: 500μs] ✅  │ │
│  │ VWAP/100K trades:     80μs  [vs Java: 120μs] ✅  │ │
│  │ Relation groupBy/100K: 30μs [vs Java: 50μs]  ✅  │ │
│  │ Cold start:           2.5s  [vs Java: 40s]    ✅  │ │
│  │ Peak memory:          100MB [vs Java: 1GB]    ✅  │ │
│  └──────────────────────────────────────────────────┘ │
│                                                       │
│  ┌─ Trend ──────────────────────────────────────────┐ │
│  │ foldPut/10K:  ▅▅▅▄▃▃▂▂▁▁  (improving)          │ │
│  │ VWAP/100K:    ▅▅▅▅▄▃▃▂▂▂  (improving)          │ │
│  │ cold start:   ▅▅▄▄▃▃▃▃▃▃  (stable)             │ │
│  └──────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────┘
```

---

## Phased Rollout

| Phase | Benchmarks Available | Why Then |
|---|---|---|
| **Phase 1: Value + Heap** | Micro: value ops, property access | First code exists |
| **Phase 2: Interpreter** | Micro: dispatch + Scenario: fib, fold | Can execute Pure |
| **Phase 3: Collections** | Scenario: foldPut, collection ops | HAMT integrated |
| **Phase 4: Relation** | Scenario: groupBy, filter, join | Relation engine built |
| **Phase 5: Compiled** | Micro: struct access + Scenario: VWAP | Codegen tool working |
| **Phase 6: Parity** | Tier 3: full Rust vs Java comparison | Feature-complete |

---

## Golden Rules

1. **Benchmark the claim, not the code.** If we claim "HAMT is 100x faster for fold+put," the benchmark must show exactly that comparison.

2. **Always include the baseline.** A number without context is meaningless. Every benchmark shows: interpreted, compiled, native, and Java-equivalent.

3. **Measure wall clock AND allocations.** Use `#[global_allocator]` with a counting allocator to track allocation counts. GC-free doesn't help if we allocate 10x more.

4. **Pin the hardware.** CI benchmarks run on the same machine class. Local benchmarks document CPU model and OS version.

5. **Never benchmark debug builds.** Always `cargo bench` (which uses `--release`). Debug Rust is 10-50x slower than release.

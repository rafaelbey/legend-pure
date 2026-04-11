# Legend Pure Parser — Stress Tests

Synthetic model generation and pipeline stress testing at scale using `criterion.rs`.

This crate generates Pure grammar source for models of varying size
(1K → 100K classes) and runs them through the full parse → compile →
compose pipeline. It measures precise, statistical timing for each phase and uses a custom tracking allocator to report the exact peak memory allocation delta.

## Test Tiers

| Tier | Classes | Feature Gate | Use-case |
|------|---------|-------------|----------|
| `1K hub-spoke` | 1,000 | default | Fast baseline regression checks |
| `10K hub-spoke` | 10,000 | default | Scaling curve checks |
| `10K dense` | 10,000 | default | Testing graph connectivity impact |
| `100K hub-spoke` | 100,000 | `heavy` | High-scale limit testing |
| `100K chaotic` | 100,000 | `heavy` | Realistic non-uniform distribution |

## Pipeline Phases Measured
The suite isolates each stage into its own `criterion` benchmark group:
1. **`generate`**: String concatenation and source generation efficiency.
2. **`parse`**: Source string conversion to AST elements.
3. **`compile`**: PureModel build performance constraint checks.
4. **`compose_roundtrip`**: AST to syntax serialization.
5. **`path_resolution`**: O(1) query lookup performance across the model.

## Usage

**1. Running Correctness Validation (Tests)**
Runs assertions directly, checking that the generated models meet structural counts without lengthy warmup or variance profiling.

```bash
# Standard suites
cargo test -p legend-pure-parser-stress

# Include massive topologies
cargo test -p legend-pure-parser-stress --features heavy
```

**2. Running Performance Benchmarks**
Runs rigorous testing against Criterion natively and outputs Memory usage statistics to your terminal.

```bash
# Standard benchmarking 
cargo bench -p legend-pure-parser-stress --bench pipeline

# Including 100K heavy topologies (Takes a long time)
cargo bench -p legend-pure-parser-stress --bench pipeline --features heavy
```

**3. Baselines & Regressions**

Store the current implementation state on disk as the benchmark's baseline threshold so PR checks can immediately show delta timings.

```bash
# Save your current local state as 'main'
cargo bench -p legend-pure-parser-stress --bench pipeline -- --save-baseline main

# Make changes to the parser or compiler crates...

# Compare your changes cleanly
cargo bench -p legend-pure-parser-stress --bench pipeline -- --baseline main
```

Reports generate beautiful SVG/HTML breakdowns at `target/criterion/report/index.html`.

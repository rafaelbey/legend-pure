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

//! Microbenchmark for `bootstrap::create_bootstrap_chunk` cost.
//!
//! Run with `cargo run --example bootstrap_chunk_bench -p legend-pure-pure --release`.
//! Prints the cold call cost (first invocation in the process) and warm
//! distribution over 100 subsequent calls.

use std::time::Instant;

use legend_pure_parser_pure::bootstrap;
use legend_pure_parser_pure::model::PureModel;

fn main() {
    let t0 = Instant::now();
    let model = PureModel::new();
    let _ = bootstrap::create_bootstrap_chunk(model.root_package);
    let cold_us = t0.elapsed().as_micros();
    println!("cold first call: {cold_us} µs");

    let mut samples = Vec::with_capacity(100);
    for _ in 0..100 {
        let t = Instant::now();
        let model = PureModel::new();
        let _ = bootstrap::create_bootstrap_chunk(model.root_package);
        samples.push(t.elapsed().as_micros());
    }
    samples.sort_unstable();
    let min = samples[0];
    let median = samples[50];
    let p90 = samples[90];
    let max = samples[99];
    println!("warm (100 samples): min={min} µs  median={median} µs  p90={p90} µs  max={max} µs");
}

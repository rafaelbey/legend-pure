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

//! Stress test pipeline benchmarks.

#![allow(clippy::similar_names)]

use criterion::{Criterion, black_box, criterion_group, criterion_main};

use legend_pure_parser_stress::alloc;
#[cfg(feature = "heavy")]
use legend_pure_parser_stress::generate::chaotic::{self, ChaoticConfig};
use legend_pure_parser_stress::generate::hub_spoke::{self, HubSpokeConfig};
use smol_str::SmolStr;

#[global_allocator]
static ALLOC: alloc::TrackingAllocator = alloc::TrackingAllocator;

// -----------------------------------------------------------------------------
// Phase 0: Generate
// -----------------------------------------------------------------------------

fn bench_generate(c: &mut Criterion) {
    let mut group = c.benchmark_group("generate");

    group.bench_function("hub_spoke_1k", |b| {
        let config = HubSpokeConfig::standard_1k();
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| {
            black_box(hub_spoke::generate(black_box(&config)));
        });
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    group.bench_function("hub_spoke_10k", |b| {
        let config = HubSpokeConfig::standard_10k();
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| {
            black_box(hub_spoke::generate(black_box(&config)));
        });
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    group.bench_function("dense_10k", |b| {
        let config = HubSpokeConfig::dense_10k();
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| {
            black_box(hub_spoke::generate(black_box(&config)));
        });
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    #[cfg(feature = "heavy")]
    group.bench_function("hub_spoke_100k", |b| {
        let config = HubSpokeConfig::standard_100k();
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| {
            black_box(hub_spoke::generate(black_box(&config)));
        });
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    #[cfg(feature = "heavy")]
    group.bench_function("chaotic_100k", |b| {
        let config = ChaoticConfig::standard_100k();
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| {
            black_box(chaotic::generate(black_box(&config)));
        });
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    group.finish();
}

// -----------------------------------------------------------------------------
// Phase 1: Parse
// -----------------------------------------------------------------------------

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");

    let (src_hub_1k, _) = hub_spoke::generate(&HubSpokeConfig::standard_1k());
    group.bench_function("hub_spoke_1k", |b| {
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| legend_pure_parser_parser::parse(black_box(&src_hub_1k), "bench.pure").unwrap());
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    let (src_hub_10k, _) = hub_spoke::generate(&HubSpokeConfig::standard_10k());
    group.bench_function("hub_spoke_10k", |b| {
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| legend_pure_parser_parser::parse(black_box(&src_hub_10k), "bench.pure").unwrap());
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    let (src_dense_10k, _) = hub_spoke::generate(&HubSpokeConfig::dense_10k());
    group.bench_function("dense_10k", |b| {
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| {
            legend_pure_parser_parser::parse(black_box(&src_dense_10k), "bench.pure").unwrap()
        });
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    #[cfg(feature = "heavy")]
    {
        let (src_hub_100k, _) = hub_spoke::generate(&HubSpokeConfig::standard_100k());
        group.bench_function("hub_spoke_100k", |b| {
            let baseline_bytes = alloc::current_bytes();
            alloc::reset();
            b.iter(|| {
                legend_pure_parser_parser::parse(black_box(&src_hub_100k), "bench.pure").unwrap()
            });
            let mem = alloc::snapshot();
            println!(
                "  peak_memory_delta: {} KB",
                alloc::peak_delta(baseline_bytes) / 1024
            );
            println!("  total_allocs: {}", mem.alloc_count);
        });

        let (src_chaotic_100k, _) = chaotic::generate(&ChaoticConfig::standard_100k());
        group.bench_function("chaotic_100k", |b| {
            let baseline_bytes = alloc::current_bytes();
            alloc::reset();
            b.iter(|| {
                legend_pure_parser_parser::parse(black_box(&src_chaotic_100k), "bench.pure")
                    .unwrap()
            });
            let mem = alloc::snapshot();
            println!(
                "  peak_memory_delta: {} KB",
                alloc::peak_delta(baseline_bytes) / 1024
            );
            println!("  total_allocs: {}", mem.alloc_count);
        });
    }

    group.finish();
}

// -----------------------------------------------------------------------------
// Phase 1b: Parse Multi-File (Parallelism comparison)
// -----------------------------------------------------------------------------

fn bench_parse_multi_file(c: &mut Criterion) {
    use legend_pure_parser_parser::SourceProvider;
    use legend_pure_parser_parser::source::SourceInput;

    let mut group = c.benchmark_group("parse_multi_file");

    // Generate 1K hub-spoke as individual files (101 files: 1 preamble + 100 hubs)
    let files_1k: Vec<_> = hub_spoke::generate_files(&HubSpokeConfig::standard_1k());

    let inputs_1k: Vec<_> = files_1k
        .iter()
        .map(|(name, src)| SourceInput::in_memory(name, src))
        .collect();

    group.bench_function("hub_spoke_1k_sequential", |b| {
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| {
            let asts: Vec<_> = inputs_1k
                .iter()
                .map(|input| {
                    let text = input.source_text().unwrap();
                    legend_pure_parser_parser::parse(&text, input.name()).unwrap()
                })
                .collect();
            black_box(asts);
        });
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    group.bench_function("hub_spoke_1k_parallel", |b| {
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| {
            let results = legend_pure_parser_parser::parse_many(black_box(&inputs_1k));
            black_box(results);
        });
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    // Generate 10K hub-spoke as individual files (1001 files)
    let files_10k: Vec<_> = hub_spoke::generate_files(&HubSpokeConfig::standard_10k());

    let inputs_10k: Vec<_> = files_10k
        .iter()
        .map(|(name, src)| SourceInput::in_memory(name, src))
        .collect();

    group.bench_function("hub_spoke_10k_sequential", |b| {
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| {
            let asts: Vec<_> = inputs_10k
                .iter()
                .map(|input| {
                    let text = input.source_text().unwrap();
                    legend_pure_parser_parser::parse(&text, input.name()).unwrap()
                })
                .collect();
            black_box(asts);
        });
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    group.bench_function("hub_spoke_10k_parallel", |b| {
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| {
            let results = legend_pure_parser_parser::parse_many(black_box(&inputs_10k));
            black_box(results);
        });
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    #[cfg(feature = "heavy")]
    {
        // Generate 100K chaotic as individual files (1001 files)
        let files_100k: Vec<_> = chaotic::generate_files(&ChaoticConfig::standard_100k(), 1000);

        let inputs_100k: Vec<_> = files_100k
            .iter()
            .map(|(name, src)| SourceInput::in_memory(name, src))
            .collect();

        group.bench_function("chaotic_100k_parallel", |b| {
            let baseline_bytes = alloc::current_bytes();
            alloc::reset();
            b.iter(|| {
                let results = legend_pure_parser_parser::parse_many(black_box(&inputs_100k));
                black_box(results);
            });
            let mem = alloc::snapshot();
            println!(
                "  peak_memory_delta: {} KB",
                alloc::peak_delta(baseline_bytes) / 1024
            );
            println!("  total_allocs: {}", mem.alloc_count);
        });
    }

    group.finish();
}

// -----------------------------------------------------------------------------
// Phase 2: Compile
// -----------------------------------------------------------------------------

fn bench_compile(c: &mut Criterion) {
    let mut group = c.benchmark_group("compile");

    let (src_hub_1k, _) = hub_spoke::generate(&HubSpokeConfig::standard_1k());
    let ast_hub_1k = legend_pure_parser_parser::parse(&src_hub_1k, "bench.pure").unwrap();
    group.bench_function("hub_spoke_1k", |b| {
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| {
            legend_pure_parser_pure::compile!(black_box(std::slice::from_ref(&ast_hub_1k))).unwrap()
        });
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    let (src_hub_10k, _) = hub_spoke::generate(&HubSpokeConfig::standard_10k());
    let ast_hub_10k = legend_pure_parser_parser::parse(&src_hub_10k, "bench.pure").unwrap();
    group.bench_function("hub_spoke_10k", |b| {
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| {
            legend_pure_parser_pure::compile!(black_box(std::slice::from_ref(&ast_hub_10k)))
                .unwrap()
        });
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    let (src_dense_10k, _) = hub_spoke::generate(&HubSpokeConfig::dense_10k());
    let ast_dense_10k = legend_pure_parser_parser::parse(&src_dense_10k, "bench.pure").unwrap();
    group.bench_function("dense_10k", |b| {
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| {
            legend_pure_parser_pure::compile!(black_box(std::slice::from_ref(&ast_dense_10k)))
                .unwrap()
        });
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    #[cfg(feature = "heavy")]
    {
        let (src_hub_100k, _) = hub_spoke::generate(&HubSpokeConfig::standard_100k());
        let ast_hub_100k = legend_pure_parser_parser::parse(&src_hub_100k, "bench.pure").unwrap();
        // Set sample size to 10 for 100K compile it may be slow
        group.sample_size(10);
        group.bench_function("hub_spoke_100k", |b| {
            let baseline_bytes = alloc::current_bytes();
            alloc::reset();
            b.iter(|| {
                legend_pure_parser_pure::compile!(black_box(std::slice::from_ref(&ast_hub_100k)))
                    .unwrap()
            });
            let mem = alloc::snapshot();
            println!(
                "  peak_memory_delta: {} KB",
                alloc::peak_delta(baseline_bytes) / 1024
            );
            println!("  total_allocs: {}", mem.alloc_count);
        });

        let (src_chaotic_100k, _) = chaotic::generate(&ChaoticConfig::standard_100k());
        let ast_chaotic_100k =
            legend_pure_parser_parser::parse(&src_chaotic_100k, "bench.pure").unwrap();
        group.bench_function("chaotic_100k", |b| {
            let baseline_bytes = alloc::current_bytes();
            alloc::reset();
            b.iter(|| {
                legend_pure_parser_pure::compile!(black_box(std::slice::from_ref(
                    &ast_chaotic_100k
                )))
                .unwrap()
            });
            let mem = alloc::snapshot();
            println!(
                "  peak_memory_delta: {} KB",
                alloc::peak_delta(baseline_bytes) / 1024
            );
            println!("  total_allocs: {}", mem.alloc_count);
        });
    }

    group.finish();
}

// -----------------------------------------------------------------------------
// Phase 3: Compose Roundtrip
// -----------------------------------------------------------------------------

fn bench_compose(c: &mut Criterion) {
    let mut group = c.benchmark_group("compose_roundtrip");

    let (src_hub_1k, _) = hub_spoke::generate(&HubSpokeConfig::standard_1k());
    let ast_hub_1k = legend_pure_parser_parser::parse(&src_hub_1k, "bench.pure").unwrap();
    group.bench_function("hub_spoke_1k", |b| {
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| legend_pure_parser_compose::compose_source_file(black_box(&ast_hub_1k)));
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    let (src_hub_10k, _) = hub_spoke::generate(&HubSpokeConfig::standard_10k());
    let ast_hub_10k = legend_pure_parser_parser::parse(&src_hub_10k, "bench.pure").unwrap();
    group.bench_function("hub_spoke_10k", |b| {
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| legend_pure_parser_compose::compose_source_file(black_box(&ast_hub_10k)));
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    let (src_dense_10k, _) = hub_spoke::generate(&HubSpokeConfig::dense_10k());
    let ast_dense_10k = legend_pure_parser_parser::parse(&src_dense_10k, "bench.pure").unwrap();
    group.bench_function("dense_10k", |b| {
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| legend_pure_parser_compose::compose_source_file(black_box(&ast_dense_10k)));
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    #[cfg(feature = "heavy")]
    {
        let (src_hub_100k, _) = hub_spoke::generate(&HubSpokeConfig::standard_100k());
        let ast_hub_100k = legend_pure_parser_parser::parse(&src_hub_100k, "bench.pure").unwrap();
        group.sample_size(10);
        group.bench_function("hub_spoke_100k", |b| {
            let baseline_bytes = alloc::current_bytes();
            alloc::reset();
            b.iter(|| legend_pure_parser_compose::compose_source_file(black_box(&ast_hub_100k)));
            let mem = alloc::snapshot();
            println!(
                "  peak_memory_delta: {} KB",
                alloc::peak_delta(baseline_bytes) / 1024
            );
            println!("  total_allocs: {}", mem.alloc_count);
        });

        let (src_chaotic_100k, _) = chaotic::generate(&ChaoticConfig::standard_100k());
        let ast_chaotic_100k =
            legend_pure_parser_parser::parse(&src_chaotic_100k, "bench.pure").unwrap();
        group.bench_function("chaotic_100k", |b| {
            let baseline_bytes = alloc::current_bytes();
            alloc::reset();
            b.iter(|| {
                legend_pure_parser_compose::compose_source_file(black_box(&ast_chaotic_100k))
            });
            let mem = alloc::snapshot();
            println!(
                "  peak_memory_delta: {} KB",
                alloc::peak_delta(baseline_bytes) / 1024
            );
            println!("  total_allocs: {}", mem.alloc_count);
        });
    }

    group.finish();
}

// -----------------------------------------------------------------------------
// Phase 3b: Compose Multi-File (Parallelism comparison)
// -----------------------------------------------------------------------------

fn bench_compose_multi_file(c: &mut Criterion) {
    let mut group = c.benchmark_group("compose_multi_file");

    #[cfg(feature = "heavy")]
    {
        use legend_pure_parser_parser::source::SourceInput;

        let files_100k: Vec<_> = chaotic::generate_files(&ChaoticConfig::standard_100k(), 1000);
        let inputs_100k: Vec<_> = files_100k
            .iter()
            .map(|(name, src)| SourceInput::in_memory(name, src))
            .collect();

        // First parse in parallel to get the ast pool
        let outputs = legend_pure_parser_parser::parse_many(&inputs_100k);
        let asts: Vec<_> = outputs.iter().map(|o| o.ast().unwrap()).collect();

        group.bench_function("chaotic_100k_parallel", |b| {
            let baseline_bytes = alloc::current_bytes();
            alloc::reset();
            b.iter(|| {
                let results = legend_pure_parser_compose::compose_many(black_box(&asts));
                black_box(results);
            });
            let mem = alloc::snapshot();
            println!(
                "  peak_memory_delta: {} KB",
                alloc::peak_delta(baseline_bytes) / 1024
            );
            println!("  total_allocs: {}", mem.alloc_count);
        });
    }

    group.finish();
}

// -----------------------------------------------------------------------------
// Phase 5: Path Resolution
// -----------------------------------------------------------------------------

fn bench_path_resolution(c: &mut Criterion) {
    let mut group = c.benchmark_group("path_resolution");

    let (src_hub_1k, _) = hub_spoke::generate(&HubSpokeConfig::standard_1k());
    let ast_hub_1k = legend_pure_parser_parser::parse(&src_hub_1k, "bench.pure").unwrap();
    let model_1k = legend_pure_parser_pure::compile!(&[ast_hub_1k]).unwrap();
    let paths_to_resolve_1k: Vec<[SmolStr; 2]> = (0..100)
        .map(|i| [SmolStr::new("test"), SmolStr::new(format!("H{i}"))])
        .collect();

    group.bench_function("hub_spoke_1k", |b| {
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| {
            for p in &paths_to_resolve_1k {
                black_box(model_1k.resolve_by_path(p));
            }
        });
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    let config_10k = HubSpokeConfig::standard_10k();
    let (src_hub_10k, _) = hub_spoke::generate(&config_10k);
    let ast_hub_10k = legend_pure_parser_parser::parse(&src_hub_10k, "bench.pure").unwrap();
    let model_10k = legend_pure_parser_pure::compile!(&[ast_hub_10k]).unwrap();
    let paths_to_resolve_10k: Vec<[SmolStr; 2]> = (0..config_10k.hubs.min(1000))
        .map(|i| [SmolStr::new("test"), SmolStr::new(format!("H{i}"))])
        .collect();

    group.bench_function("hub_spoke_10k", |b| {
        let baseline_bytes = alloc::current_bytes();
        alloc::reset();
        b.iter(|| {
            for p in &paths_to_resolve_10k {
                black_box(model_10k.resolve_by_path(p));
            }
        });
        let mem = alloc::snapshot();
        println!(
            "  peak_memory_delta: {} KB",
            alloc::peak_delta(baseline_bytes) / 1024
        );
        println!("  total_allocs: {}", mem.alloc_count);
    });

    #[cfg(feature = "heavy")]
    {
        let config_100k = HubSpokeConfig::standard_100k();
        let (src_hub_100k, _) = hub_spoke::generate(&config_100k);
        let ast_hub_100k = legend_pure_parser_parser::parse(&src_hub_100k, "bench.pure").unwrap();
        let model_100k = legend_pure_parser_pure::compile!(&[ast_hub_100k]).unwrap();
        let paths_to_resolve_100k: Vec<[SmolStr; 2]> = (0..config_100k.hubs.min(1000))
            .map(|i| [SmolStr::new("test"), SmolStr::new(format!("H{i}"))])
            .collect();

        group.bench_function("hub_spoke_100k", |b| {
            let baseline_bytes = alloc::current_bytes();
            alloc::reset();
            b.iter(|| {
                for p in &paths_to_resolve_100k {
                    black_box(model_100k.resolve_by_path(p));
                }
            });
            let mem = alloc::snapshot();
            println!(
                "  peak_memory_delta: {} KB",
                alloc::peak_delta(baseline_bytes) / 1024
            );
            println!("  total_allocs: {}", mem.alloc_count);
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_generate,
    bench_parse,
    bench_parse_multi_file,
    bench_compile,
    bench_compose,
    bench_compose_multi_file,
    bench_path_resolution
);
criterion_main!(benches);

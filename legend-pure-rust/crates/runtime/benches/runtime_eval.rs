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

//! End-to-end eval benchmarks for runtime hot paths.
//!
//! Run: `cargo bench --bench runtime_eval`
//!
//! Complements `runtime_micro` (which exercises raw heap / Value / Decimal
//! / Date primitives) by driving full pipelines through the
//! `Evaluator` — function dispatch, property access, lambda
//! invocation, and the addColumns chain. Catches regressions in
//! whole-pipeline cost that wouldn't show up in primitive benchmarks
//! (e.g. lambda-param inference re-running per call, or platform
//! re-parse on every Evaluator construction).

use std::sync::OnceLock;

use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};

use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::value::Value;
use smol_str::SmolStr;

// ---------------------------------------------------------------------------
// Shared platform fixture — parse + compile once, reuse across iterations
// ---------------------------------------------------------------------------

struct Fixture {
    parsed_files: Vec<SourceFile>,
    auto_imports: Vec<SmolStr>,
}

fn fixture() -> &'static Fixture {
    static FIX: OnceLock<Fixture> = OnceLock::new();
    FIX.get_or_init(|| {
        let repos = legend_pure_core_platform::repo::Repo::default_embedded();
        let mut parsed_files = Vec::new();
        for repo in &repos {
            for (content, path) in repo.sources() {
                match legend_pure_parser_parser::parse_with_islands(
                    content,
                    path,
                    legend_pure_dsl_graph::parser::default_island_parsers(),
                ) {
                    Ok(sf) => parsed_files.push(sf),
                    Err(partial) => parsed_files.push(partial.source_file),
                }
            }
        }
        let auto_imports = legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
            .iter()
            .map(|&s| SmolStr::new(s))
            .collect();
        Fixture {
            parsed_files,
            auto_imports,
        }
    })
}

/// Compile platform + the supplied user source. Mirrors
/// `eval_tests::compile_with_platform` so bench compile-time matches
/// runtime-test compile-time.
fn compile_user(source: &str) -> PureModel {
    let fix = fixture();
    let user_ast = legend_pure_parser_parser::parse_with_islands(
        source,
        "<bench>",
        legend_pure_dsl_graph::parser::default_island_parsers(),
    )
    .expect("user source should parse");
    let mut all_files = fix.parsed_files.clone();
    all_files.push(user_ast);
    match legend_pure_parser_pure::pipeline::compile(&all_files, &fix.auto_imports) {
        Ok(m) => m,
        Err(partial) => partial.model,
    }
}

// ---------------------------------------------------------------------------
// Function call dispatch — cost of `Evaluator::call_user_function_by_id`
// for the smallest possible function body.
// ---------------------------------------------------------------------------

fn bench_function_call(c: &mut Criterion) {
    let mut group = c.benchmark_group("eval_function_call");

    let model = compile_user(
        r"
        function test::add_one(x: Integer[1]): Integer[1]
        {
            $x + 1
        }

        function test::trivial(): Integer[1]
        {
            42
        }
        ",
    );
    let registry = NativeRegistry::standard();

    // `iter_batched` constructs a fresh Evaluator per iteration in
    // *setup* (not measured) and only times the call itself — so the
    // ~300µs cost of `bootstrap_metamodel` walking 1300+ M3 elements
    // doesn't dominate every measurement. The cost-of-Evaluator-setup
    // is itself benchmarked in `eval_setup` below for tracking.

    group.bench_function("trivial_return_literal", |b| {
        let fn_id = model
            .resolve_by_fqn(&["test".into(), "trivial__Integer_1_".into()])
            .expect("test::trivial");
        b.iter_batched(
            || Evaluator::new(&model, &registry),
            |mut eval| black_box(eval.call_user_function_by_id(fn_id).unwrap()),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("call_with_one_int_arg", |b| {
        b.iter_batched(
            || Evaluator::new(&model, &registry),
            |mut eval| {
                black_box(
                    eval.call("test::add_one", &[Value::Integer(black_box(41))])
                        .unwrap(),
                )
            },
            BatchSize::SmallInput,
        );
    });

    // Lock the cost-of-setup as a separate metric — easy to spot if a
    // change inflates `Evaluator::new` (currently the floor for every
    // other bench).
    group.bench_function("evaluator_setup_only", |b| {
        b.iter(|| black_box(Evaluator::new(&model, &registry)));
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Property access — the unified accessor through Evaluator (Step 3).
// Exercises `eval_property_access`'s heap-row read for Value::Object
// targets (the hot path for `$obj.prop` in user code).
// ---------------------------------------------------------------------------

fn bench_property_access(c: &mut Criterion) {
    let mut group = c.benchmark_group("eval_property_access");

    let model = compile_user(
        r"
        Class test::Person
        {
            firstName: String[1];
            lastName: String[1];
            age: Integer[1];
        }

        function test::read_first(): String[1]
        {
            ^test::Person(firstName='Pierre', lastName='Doe', age=42).firstName
        }

        function test::read_chain(): Integer[1]
        {
            let p = ^test::Person(firstName='Pierre', lastName='Doe', age=42);
            $p.age
        }
        ",
    );
    let registry = NativeRegistry::standard();

    group.bench_function("read_property_inline_construct", |b| {
        b.iter_batched(
            || Evaluator::new(&model, &registry),
            |mut eval| black_box(eval.call("test::read_first", &[]).unwrap()),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("read_via_let_binding", |b| {
        b.iter_batched(
            || Evaluator::new(&model, &registry),
            |mut eval| black_box(eval.call("test::read_chain", &[]).unwrap()),
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Lambda invocation — `map` over a small collection. Locks in the cost
// of the `lower_args_with_lambda_inference` narrowing path that
// commit b4ff09432a5 introduced.
// ---------------------------------------------------------------------------

fn bench_lambda(c: &mut Criterion) {
    let mut group = c.benchmark_group("eval_lambda");

    let model = compile_user(
        r"
        function test::map_double(): Integer[*]
        {
            [1, 2, 3, 4, 5]->map(x| $x * 2)
        }

        function test::map_concat(): String[1]
        {
            ['a', 'b', 'c']->map(s| $s + 'X')->joinStrings(',')
        }

        function test::map_filter_chain(): Integer[*]
        {
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
                ->filter(n| $n > 3)
                ->map(n| $n * $n)
        }
        ",
    );
    let registry = NativeRegistry::standard();

    group.bench_function("map_double_5", |b| {
        b.iter_batched(
            || Evaluator::new(&model, &registry),
            |mut eval| black_box(eval.call("test::map_double", &[]).unwrap()),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("map_concat_typed_lambda_param", |b| {
        b.iter_batched(
            || Evaluator::new(&model, &registry),
            |mut eval| black_box(eval.call("test::map_concat", &[]).unwrap()),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("filter_then_map", |b| {
        b.iter_batched(
            || Evaluator::new(&model, &registry),
            |mut eval| black_box(eval.call("test::map_filter_chain", &[]).unwrap()),
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// addColumns chain — exercises the relation literal lowering, the
// `@(x:String)->genericType().rawType->cast(...)->toOne()` chain, and
// the `addColumns` native we landed for surveyor 211/0/0.
// ---------------------------------------------------------------------------

fn bench_add_columns(c: &mut Criterion) {
    let mut group = c.benchmark_group("eval_relation");

    let model = compile_user(
        r"
        function test::add_two_columns(): Integer[1]
        {
            let rt = addColumns(
                @(x:String)->genericType().rawType->cast(@RelationType<Any>)->toOne(),
                ~[ab:String[1], z:Integer]);
            $rt.columns->size()
        }
        ",
    );
    let registry = NativeRegistry::standard();

    group.bench_function("addColumns_2cols_to_1col_source", |b| {
        b.iter_batched(
            || Evaluator::new(&model, &registry),
            |mut eval| black_box(eval.call("test::add_two_columns", &[]).unwrap()),
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Recursive function dispatch — exercises `call_user_function`'s
// per-invocation cost (parameter binding + body evaluation). The
// review identified body/parameters cloning as the hottest path in
// the interpreter; a recursive Fibonacci pins the cost so the
// `Rc<[ValueSpec]>` refactor can be measured against this baseline.
// ---------------------------------------------------------------------------

fn bench_recursion(c: &mut Criterion) {
    let mut group = c.benchmark_group("eval_recursion");

    let model = compile_user(
        r"
        function test::fib(n: Integer[1]): Integer[1]
        {
            if($n < 2, | $n, | test::fib($n - 1) + test::fib($n - 2))
        }

        function test::sum_to(n: Integer[1]): Integer[1]
        {
            if($n <= 0, | 0, | $n + test::sum_to($n - 1))
        }
        ",
    );
    let registry = NativeRegistry::standard();

    // fib(15) = 610 — ~1973 calls, big enough to surface clone cost
    // without taking long enough to slow the bench loop.
    group.bench_function("fib_15", |b| {
        b.iter_batched(
            || Evaluator::new(&model, &registry),
            |mut eval| {
                black_box(
                    eval.call("test::fib", &[Value::Integer(black_box(15))])
                        .unwrap(),
                )
            },
            BatchSize::SmallInput,
        );
    });

    // Linear recursion to 50 — isolates per-call overhead from the
    // exponential branching of fib.
    group.bench_function("sum_to_50", |b| {
        b.iter_batched(
            || Evaluator::new(&model, &registry),
            |mut eval| {
                black_box(
                    eval.call("test::sum_to", &[Value::Integer(black_box(50))])
                        .unwrap(),
                )
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    bench_function_call,
    bench_property_access,
    bench_lambda,
    bench_add_columns,
    bench_recursion,
);
criterion_main!(benches);

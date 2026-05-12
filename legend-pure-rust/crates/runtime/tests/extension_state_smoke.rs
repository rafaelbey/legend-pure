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

//! Smoke tests for per-evaluator typed extension state.
//!
//! The store itself has unit tests in `crates/runtime/src/extensions.rs`.
//! This file validates the wiring from `Evaluator` outward — that the
//! field is initialised in every constructor, exposed on the public
//! API, and that two evaluators get independent state.
//
// `PureException` is intentionally rich (carries a call stack) so the
// `Result<T, PureException>` return type used by `get_or_init` lights up
// `clippy::result_large_err`. The library crate allows this lint
// crate-wide; mirror that here for tests that thread the same Result
// type through closures.
#![allow(clippy::result_large_err)]

use std::cell::RefCell;

use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::eval::Evaluator;

/// Per-extension state with interior mutability — mirrors the shape
/// real extensions (e.g. `DuckDBState`) will adopt.
struct CounterState {
    n: RefCell<u32>,
}

#[test]
fn fresh_evaluator_has_empty_extension_store() {
    let model = PureModel::new();
    let eval = Evaluator::new_default(&model);
    assert_eq!(eval.extensions().len(), 0);
    assert!(eval.extensions().is_empty());
}

#[test]
fn get_or_init_installs_state_and_subsequent_calls_reuse_it() {
    let model = PureModel::new();
    let eval = Evaluator::new_default(&model);

    {
        let s = eval
            .extensions()
            .get_or_init::<CounterState, _>(|| Ok(CounterState { n: RefCell::new(0) }))
            .expect("first init");
        *s.n.borrow_mut() = 7;
    }

    // Second call must not re-run the init closure; we'd see `n == 0` if it did.
    let s = eval
        .extensions()
        .get_or_init::<CounterState, _>(|| {
            panic!("init should run exactly once per Evaluator per type");
        })
        .expect("second access");
    assert_eq!(*s.n.borrow(), 7);
    assert_eq!(eval.extensions().len(), 1);
}

#[test]
fn distinct_evaluators_get_independent_state() {
    let model = PureModel::new();
    let eval_a = Evaluator::new_default(&model);
    let eval_b = Evaluator::new_default(&model);

    {
        let s = eval_a
            .extensions()
            .get_or_init::<CounterState, _>(|| Ok(CounterState { n: RefCell::new(0) }))
            .expect("A init");
        *s.n.borrow_mut() = 11;
    }

    // Eval B's store is independent — it should not see A's value.
    let s_b = eval_b
        .extensions()
        .get_or_init::<CounterState, _>(|| Ok(CounterState { n: RefCell::new(0) }))
        .expect("B init");
    assert_eq!(*s_b.n.borrow(), 0, "B's state must be independent of A's");
}

#[test]
fn state_drops_with_the_evaluator() {
    use std::rc::Rc;

    struct DropTracker {
        flag: Rc<RefCell<bool>>,
    }
    impl Drop for DropTracker {
        fn drop(&mut self) {
            *self.flag.borrow_mut() = true;
        }
    }

    let flag = Rc::new(RefCell::new(false));
    let flag_inner = Rc::clone(&flag);

    let model = PureModel::new();
    let eval = Evaluator::new_default(&model);
    let _ = eval
        .extensions()
        .get_or_init::<DropTracker, _>(|| {
            Ok(DropTracker {
                flag: Rc::clone(&flag_inner),
            })
        })
        .expect("install");
    assert!(!*flag.borrow(), "still alive while evaluator holds it");

    drop(eval);
    assert!(*flag.borrow(), "state must drop with the Evaluator");
}

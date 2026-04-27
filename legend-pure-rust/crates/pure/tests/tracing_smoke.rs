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

//! Smoke test: verify pipeline pass instrumentation produces the expected
//! span waterfall when a tracing subscriber is installed.
//!
//! Locks `crates/pure/BACKLOG.md` "Compilation tracing" P1.

use std::sync::{Arc, Mutex};

use tracing::span::{Attributes, Id};
use tracing::{Event, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::Registry;

#[derive(Default)]
struct SpanCollector {
    spans: Arc<Mutex<Vec<String>>>,
}

impl<S: Subscriber> Layer<S> for SpanCollector {
    fn enabled(&self, _metadata: &Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        true
    }

    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {
        if let Ok(mut g) = self.spans.lock() {
            g.push(attrs.metadata().name().to_string());
        }
    }

    fn on_event(&self, _event: &Event<'_>, _ctx: Context<'_, S>) {}
}

#[test]
fn pipeline_passes_emit_spans() {
    let collected: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let layer = SpanCollector {
        spans: Arc::clone(&collected),
    };
    let subscriber = Registry::default().with(layer);

    let _guard = tracing::subscriber::set_default(subscriber);

    // Trivially small program — exercises the whole pipeline without
    // pulling in platform sources.
    let sf = legend_pure_parser_parser::parse("Class Person { name: String[1]; }", "smoke.pure")
        .expect("parse");

    let _ = legend_pure_parser_pure::pipeline::compile(&[sf], &[]);

    let captured = collected.lock().expect("lock spans").clone();
    let want = [
        "compile",
        "pass_declare",
        "pass_topo_sort",
        "pass_define_signatures",
        "pass_define_bodies",
        "pass_define_class_bodies",
        "pass_infer",
        "validate",
    ];
    for name in want {
        assert!(
            captured.iter().any(|s| s == name),
            "missing span '{name}' in {captured:?}",
        );
    }
}

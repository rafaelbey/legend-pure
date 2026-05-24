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

//! JSON-shaped, navigable rendering of [`Value`]s for IDE debuggers.
//!
//! Embedders (DAP, LSP) call [`render_locals`] to turn a snapshot of
//! variable bindings into a flat tree of owned-string nodes. The tree
//! is keyed by integer `child_ref` IDs that mirror the DAP
//! `variablesReference` protocol — the consumer hands them back through
//! `variables(reference)` requests and we look up children in O(1).
//!
//! # Why a tree of strings (not borrowed [`Value`]s)?
//!
//! The runtime's [`Value`] is `!Send` — it carries
//! `Rc<RefCell<HeapEntry>>` heap handles. Debuggers that pause on one
//! thread and serve requests from another (DAP `Arc<Mutex<…>>` shared
//! state) cannot cross [`Value`]s over the thread boundary. The
//! renderer materialises the full tree to owned strings on the eval
//! thread *during the pause hook*; the resulting `DisplayTree` is
//! `Send`.
//!
//! # Rendering strategy
//!
//! - **Leaves** (`Integer`, `String`, `Date`, `EnumValue`, `Element`,
//!   `Function`, `UnitInstance`, `Unit`) — render via the existing
//!   `meta::pure::functions::string::toRepresentation` native. Routing
//!   through the evaluator's `call_function` keeps a single source of
//!   truth: any future divergence becomes a `toRepresentation` bug, not
//!   a debugger bug.
//! - **Objects** — enumerate properties via a reflective call to
//!   `meta::pure::functions::meta::properties($obj->genericType())`,
//!   so the property list (own + association + inherited) comes from
//!   the same Pure function user code calls. Property values are
//!   read directly from the heap by name once we know the names.
//!   Cycles are detected via `Rc::as_ptr`. The renderer's reflective
//!   calls go through the evaluator's `EvalContext`; the
//!   `rendering_depth` guard on the evaluator (see `Evaluator::eval`)
//!   prevents those nested evaluations from re-triggering the pause
//!   hook.
//! - **Collections / Maps** — iterate elements; each entry becomes one
//!   indexed child. Past `collection_breadth_cap` entries we emit a
//!   `[…truncated N]` sentinel child.
//!
//! # Depth + breadth caps
//!
//! [`DisplayConfig::depth_cap`] (default 64) collapses to a leaf
//! `<depth limit>`; [`DisplayConfig::collection_breadth_cap`] (default
//! 200) collapses the tail of long collections / maps.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use im_rc::Vector as PVector;
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};
use smol_str::SmolStr;

use crate::heap::{HeapEntry, RuntimeHeap};
use crate::native::EvalContextTrait;
use crate::value::{FunctionValue, MapState, Value, ValueKey};

/// One node in the rendered tree.
///
/// `child_ref == 0` denotes a leaf — the consumer should not issue a
/// `variables(0)` request. Otherwise the ref selects a child list in
/// [`DisplayTree::nodes`].
#[derive(Debug, Clone)]
pub struct DisplayNode {
    /// Variable name (`f`, `propA`, `[0]`, `m[key]`).
    pub name: String,
    /// Pure-syntax inline form for the value column.
    pub value: String,
    /// Pure type + multiplicity label (`Integer[1]`, `abc::Class1[1]`).
    /// `None` when the renderer can't recover a stable label (rare —
    /// only when classifier lookup fails).
    pub r#type: Option<String>,
    /// Lookup key for children, or `0` if this is a leaf.
    pub child_ref: i64,
    /// Object/map child count when present, for the DAP
    /// `namedVariables` field.
    pub named_count: Option<i64>,
    /// Collection child count when present, for the DAP
    /// `indexedVariables` field.
    pub indexed_count: Option<i64>,
}

/// A rendered variable tree.
///
/// `root` is the ref for the top-level "Locals" scope; pass it to the
/// DAP `variables(root)` request to get the locals list. Container
/// nodes carry their own `child_ref`; passing those back returns each
/// container's children.
#[derive(Debug, Clone, Default)]
pub struct DisplayTree {
    /// Ref of the locals (root) child list.
    pub root: i64,
    /// All child lists, keyed by `child_ref`.
    pub nodes: HashMap<i64, Vec<DisplayNode>>,
    next_ref: i64,
}

impl DisplayTree {
    /// Allocate a fresh child ref. Refs start at 1; 0 is reserved for
    /// "no children".
    pub fn alloc_ref(&mut self) -> i64 {
        self.next_ref += 1;
        self.next_ref
    }
}

/// Knobs for the renderer. Defaults mirror what most IDEs handle
/// gracefully.
#[derive(Debug, Clone, Copy)]
pub struct DisplayConfig {
    /// Maximum recursion depth before collapsing a node to
    /// `"<depth limit>"`. Default 64.
    pub depth_cap: usize,
    /// Maximum number of indexed children rendered per collection /
    /// map. Beyond this we emit a single `[…truncated N]` sentinel.
    /// Default 200.
    pub collection_breadth_cap: usize,
    /// Maximum length (in characters) of a parent node's inline
    /// summary before it collapses to `^Class(...)` or `[...]`.
    /// Default 120.
    pub inline_summary_cap: usize,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            depth_cap: 64,
            collection_breadth_cap: 200,
            inline_summary_cap: 120,
        }
    }
}

/// FQN of the platform `toRepresentation` native — the renderer's
/// single source of truth for leaf value text.
const TO_REPRESENTATION_FQN: &str = "meta::pure::functions::string::toRepresentation";

/// FQN of `genericType(Any[1]):GenericType[1]` — produces the
/// GenericType we feed into `properties` for object reflection.
const GENERIC_TYPE_FQN: &str = "meta::pure::functions::meta::genericType";

/// FQN of `properties(GenericType[1]):Property<Nil,Any|*>[*]` — the
/// Pure-canonical property walk (own + association + inherited).
const PROPERTIES_FQN: &str = "meta::pure::functions::meta::properties";

/// Render a snapshot of `(name, value)` bindings to a navigable tree.
///
/// The result owns every string — once returned, the caller can move it
/// to another thread (e.g. into `Arc<Mutex<SessionState>>` for DAP)
/// even though [`Value`] itself is `!Send`.
///
/// Re-entry: this routine calls back into the evaluator via
/// `ctx.call_function` to invoke `toRepresentation`. Callers must
/// ensure the evaluator has set its rendering guard so the renderer's
/// own evaluations don't trigger the pause hook recursively (see
/// `Evaluator::eval`).
pub fn render_locals<'a, I>(
    ctx: &mut dyn EvalContextTrait,
    bindings: I,
    cfg: DisplayConfig,
) -> DisplayTree
where
    I: IntoIterator<Item = (&'a SmolStr, &'a Value)>,
{
    let mut tree = DisplayTree::default();
    let root_ref = tree.alloc_ref();
    tree.root = root_ref;

    let mut renderer = Renderer {
        ctx,
        tree: &mut tree,
        cfg,
        visited: HashSet::new(),
        to_repr_fn: None,
        generic_type_fn: None,
        properties_fn: None,
    };

    let mut local_nodes: Vec<DisplayNode> = bindings
        .into_iter()
        .map(|(name, value)| renderer.render_node(name.as_str(), value, 0))
        .collect();
    // Stable order — Variables panels render in send order.
    local_nodes.sort_by(|a, b| a.name.cmp(&b.name));
    tree.nodes.insert(root_ref, local_nodes);
    tree
}

struct Renderer<'a, 'b> {
    ctx: &'a mut dyn EvalContextTrait,
    tree: &'b mut DisplayTree,
    cfg: DisplayConfig,
    /// Set of `Rc::as_ptr(handle)` values currently in the recursion
    /// stack — when we re-enter the same heap object we emit
    /// `<cycle>` instead of recursing.
    visited: HashSet<*const std::cell::RefCell<HeapEntry>>,
    /// Cached `Value::Element` of `toRepresentation`, resolved on
    /// first use.
    to_repr_fn: Option<Value>,
    /// Cached `Value::Element` of `genericType`, resolved on first
    /// use.
    generic_type_fn: Option<Value>,
    /// Cached `Value::Element` of `properties`, resolved on first
    /// use.
    properties_fn: Option<Value>,
}

impl Renderer<'_, '_> {
    fn render_node(&mut self, name: &str, value: &Value, depth: usize) -> DisplayNode {
        if depth >= self.cfg.depth_cap {
            return DisplayNode {
                name: name.to_string(),
                value: "<depth limit>".to_string(),
                r#type: None,
                child_ref: 0,
                named_count: None,
                indexed_count: None,
            };
        }
        match value {
            Value::Object(handle) => self.render_object(name, handle, depth),
            Value::Collection(items) => self.render_collection(name, items, depth),
            Value::Map(state) => self.render_map(name, state, depth),
            _ => self.render_leaf(name, value),
        }
    }

    fn render_leaf(&mut self, name: &str, value: &Value) -> DisplayNode {
        DisplayNode {
            name: name.to_string(),
            value: self.leaf_text(value),
            r#type: type_label_for(self.ctx.model(), self.ctx.heap(), value),
            child_ref: 0,
            named_count: None,
            indexed_count: None,
        }
    }

    /// Render a leaf via `toRepresentation`. Falls back to the value's
    /// built-in `Display` if the native call fails — degraded but
    /// never panics.
    fn leaf_text(&mut self, value: &Value) -> String {
        let Some(callable) = self.resolve_to_repr() else {
            return fallback_render(value);
        };
        match self
            .ctx
            .call_function(&callable, std::slice::from_ref(value))
        {
            Ok(ref v) => match v {
                Value::String(s) => s.to_string(),
                other => fallback_render(other),
            },
            Err(_) => fallback_render(value),
        }
    }

    fn resolve_to_repr(&mut self) -> Option<Value> {
        self.resolve_cached(TO_REPRESENTATION_FQN, |r| &mut r.to_repr_fn)
    }

    fn render_object(
        &mut self,
        name: &str,
        handle: &crate::heap::ObjectHandle,
        depth: usize,
    ) -> DisplayNode {
        let ptr = Rc::as_ptr(handle);
        let classifier = handle.borrow().classifier();
        let class_label = classifier.to_string();
        let type_label = Some(format!("{class_label}[1]"));

        if !self.visited.insert(ptr) {
            return DisplayNode {
                name: name.to_string(),
                value: format!("^{class_label}(... <cycle>)"),
                r#type: type_label,
                child_ref: 0,
                named_count: None,
                indexed_count: None,
            };
        }

        // Single source of truth for the property walk: invoke the
        // platform's `properties(genericType($obj))` and iterate the
        // returned list. This produces declared + inherited +
        // association-derived properties in the same order user code
        // sees them.
        let prop_names = self.reflect_property_names(handle);

        let mut children: Vec<DisplayNode> = Vec::with_capacity(prop_names.len());
        for prop_name in &prop_names {
            let prop_values = read_heap_property(handle, prop_name.as_str());
            let collapsed = collapse_to_value(prop_values);
            let node = self.render_node(prop_name.as_str(), &collapsed, depth + 1);
            children.push(node);
        }
        self.visited.remove(&ptr);

        let child_count = children.len() as i64;
        let inline = build_object_inline(&class_label, &children, self.cfg.inline_summary_cap);
        let child_ref = if children.is_empty() {
            0
        } else {
            let r = self.tree.alloc_ref();
            self.tree.nodes.insert(r, children);
            r
        };
        DisplayNode {
            name: name.to_string(),
            value: inline,
            r#type: type_label,
            child_ref,
            named_count: if child_count == 0 {
                None
            } else {
                Some(child_count)
            },
            indexed_count: None,
        }
    }

    /// Drive `properties(genericType($obj))` through the evaluator
    /// to get the canonical Pure property list. Returns an ordered
    /// list of property names; the caller reads each name out of
    /// the heap to assemble the child value.
    ///
    /// Falls back to an empty list when:
    /// - either reflection function isn't loaded in the model
    ///   (model under test, not the full platform), or
    /// - any callback fails (we never let a debugger render kill an
    ///   evaluator).
    ///
    /// The fall-back path means objects render with no children
    /// rather than panic — degraded UX is preferable to a blown
    /// debug session.
    fn reflect_property_names(&mut self, handle: &crate::heap::ObjectHandle) -> Vec<SmolStr> {
        let Some(gt_fn) = self.resolve_cached(GENERIC_TYPE_FQN, |r| &mut r.generic_type_fn) else {
            return Vec::new();
        };
        let Some(props_fn) = self.resolve_cached(PROPERTIES_FQN, |r| &mut r.properties_fn) else {
            return Vec::new();
        };

        let obj_v = Value::Object(handle.clone());
        let Ok(gt) = self.ctx.call_function(&gt_fn, std::slice::from_ref(&obj_v)) else {
            return Vec::new();
        };
        let Ok(props_v) = self.ctx.call_function(&props_fn, std::slice::from_ref(&gt)) else {
            return Vec::new();
        };

        // `properties` returns `Property<Nil,Any|*>[*]`. At runtime
        // each element is a `Value::Object` heap entry carrying the
        // property metadata (Java parity: `Property` instances on
        // the heap with a `name` slot). Empty input → `Value::Unit`;
        // single property → bare `Value::Object`; otherwise
        // `Value::Collection`.
        let mut out: Vec<SmolStr> = Vec::new();
        match &props_v {
            Value::Object(prop_handle) => {
                if let Some(name) = read_prop_name(prop_handle) {
                    out.push(name);
                }
            }
            Value::Collection(items) => {
                for item in items.iter() {
                    if let Value::Object(prop_handle) = item
                        && let Some(name) = read_prop_name(prop_handle)
                    {
                        out.push(name);
                    }
                }
            }
            // Value::Unit (empty multiplicity) — no properties; any
            // other Value shape is a Pure-level type error in
            // `properties` and we degrade gracefully by skipping.
            _ => {}
        }
        out
    }

    /// Generic cached-resolution helper for FQN → `Value::Element`
    /// lookups against the live model. Avoids paying the lookup cost
    /// once per leaf / object on a busy pause.
    ///
    /// Functions live under mangled names in the model
    /// (`toRepresentation_Any_1__String_1_`), so `resolve_fqn_str`'s
    /// exact-match path doesn't see them. We use
    /// [`PureModel::resolve_function_by_path`] for prefix-matching
    /// on the simple name first; falling back to the exact lookup
    /// keeps non-function elements (rare in this module) addressable.
    fn resolve_cached(
        &mut self,
        fqn: &str,
        slot: fn(&mut Self) -> &mut Option<Value>,
    ) -> Option<Value> {
        if let Some(v) = slot(self) {
            return Some(v.clone());
        }
        let path: Vec<SmolStr> = fqn.split("::").map(SmolStr::new).collect();
        let id = self
            .ctx
            .model()
            .resolve_function_by_path(&path)
            .or_else(|| self.ctx.model().resolve_fqn_str(fqn))?;
        let v = Value::Element(id);
        *slot(self) = Some(v.clone());
        Some(v)
    }

    fn render_collection(
        &mut self,
        name: &str,
        items: &PVector<Value>,
        depth: usize,
    ) -> DisplayNode {
        let mut children: Vec<DisplayNode> = Vec::new();
        let mut iter = items.iter().enumerate();
        for (idx, item) in iter.by_ref() {
            if idx >= self.cfg.collection_breadth_cap {
                let remaining = items.len() - idx;
                children.push(DisplayNode {
                    name: format!("[...truncated {remaining}]"),
                    value: String::new(),
                    r#type: None,
                    child_ref: 0,
                    named_count: None,
                    indexed_count: None,
                });
                break;
            }
            let child = self.render_node(&format!("[{idx}]"), item, depth + 1);
            children.push(child);
        }
        let inline = build_collection_inline(&children, self.cfg.inline_summary_cap);
        let elem_type = collection_element_type(self.ctx.model(), self.ctx.heap(), items);
        let len_text = items.len();
        let type_label = elem_type.map(|t| format!("{t}[{len_text}]"));
        let indexed_count = items.len() as i64;
        let child_ref = if children.is_empty() {
            0
        } else {
            let r = self.tree.alloc_ref();
            self.tree.nodes.insert(r, children);
            r
        };
        DisplayNode {
            name: name.to_string(),
            value: inline,
            r#type: type_label,
            child_ref,
            named_count: None,
            indexed_count: if indexed_count == 0 {
                None
            } else {
                Some(indexed_count)
            },
        }
    }

    fn render_map(
        &mut self,
        name: &str,
        state: &Rc<std::cell::RefCell<MapState>>,
        depth: usize,
    ) -> DisplayNode {
        let borrowed = state.borrow();
        let entries: Vec<(Value, Value)> = borrowed
            .entries
            .iter()
            .map(|(k, v)| (value_key_to_value(k), v.clone()))
            .collect();
        drop(borrowed);
        let mut children: Vec<DisplayNode> = Vec::new();
        for (idx, (k, v)) in entries.iter().enumerate() {
            if idx >= self.cfg.collection_breadth_cap {
                let remaining = entries.len() - idx;
                children.push(DisplayNode {
                    name: format!("[...truncated {remaining}]"),
                    value: String::new(),
                    r#type: None,
                    child_ref: 0,
                    named_count: None,
                    indexed_count: None,
                });
                break;
            }
            let key_text = self.leaf_text(k);
            children.push(self.render_node(&key_text, v, depth + 1));
        }
        let inline = build_map_inline(&children, self.cfg.inline_summary_cap);
        let named_count = entries.len() as i64;
        let child_ref = if children.is_empty() {
            0
        } else {
            let r = self.tree.alloc_ref();
            self.tree.nodes.insert(r, children);
            r
        };
        DisplayNode {
            name: name.to_string(),
            value: inline,
            r#type: Some("Map[1]".to_string()),
            child_ref,
            named_count: if named_count == 0 {
                None
            } else {
                Some(named_count)
            },
            indexed_count: None,
        }
    }
}

fn type_label_for(model: &PureModel, _heap: &RuntimeHeap, value: &Value) -> Option<String> {
    let base = match value {
        Value::Boolean(_) => "Boolean".to_string(),
        Value::Integer(_) => "Integer".to_string(),
        Value::Float(_) => "Float".to_string(),
        Value::Decimal(_) => "Decimal".to_string(),
        Value::String(_) => "String".to_string(),
        Value::Date(_) => "Date".to_string(),
        Value::Latest => "Date".to_string(),
        Value::StrictTime(_) => "StrictTime".to_string(),
        Value::EnumValue { enum_id, .. } => {
            crate::model_utils::build_element_path(model, *enum_id, "::", false)
        }
        Value::Element(id) => element_kind_label(model, *id),
        Value::Function(fv) => function_value_kind(fv),
        Value::UnitInstance { unit_id, .. } => {
            crate::model_utils::build_element_path(model, *unit_id, "::", false)
        }
        Value::Unit => return Some("Any[0]".to_string()),
        Value::Object(_) | Value::Collection(_) | Value::Map(_) => return None,
    };
    Some(format!("{base}[1]"))
}

fn element_kind_label(model: &PureModel, id: ElementId) -> String {
    match model.try_get_element(id) {
        Some(Element::Class(_)) => "Class".to_string(),
        Some(Element::Enumeration(_)) => "Enumeration".to_string(),
        Some(Element::Function(_)) => "ConcreteFunctionDefinition".to_string(),
        Some(Element::Profile(_)) => "Profile".to_string(),
        Some(Element::Association(_)) => "Association".to_string(),
        Some(Element::Measure(_)) => "Measure".to_string(),
        Some(Element::PrimitiveType(_)) => "PrimitiveType".to_string(),
        Some(Element::Unit(_)) => "Unit".to_string(),
        Some(Element::PackageableMultiplicity(_)) => "PackageableMultiplicity".to_string(),
        Some(Element::Package(_)) => "Package".to_string(),
        Some(Element::DSLInstance(_)) | None => "PackageableElement".to_string(),
    }
}

fn function_value_kind(fv: &FunctionValue) -> String {
    match fv {
        FunctionValue::Lambda(_) => "LambdaFunction".to_string(),
        FunctionValue::Compiled(_) => "Function".to_string(),
        FunctionValue::Path(_) => "Path".to_string(),
    }
}

fn collection_element_type(
    _model: &PureModel,
    _heap: &RuntimeHeap,
    items: &PVector<Value>,
) -> Option<String> {
    let mut common: Option<&'static str> = None;
    for item in items {
        let tag = simple_type_tag(item)?;
        common = Some(match common {
            None => tag,
            Some(prev) if prev == tag => prev,
            _ => "Any",
        });
    }
    common.map(str::to_string)
}

fn simple_type_tag(value: &Value) -> Option<&'static str> {
    Some(match value {
        Value::Boolean(_) => "Boolean",
        Value::Integer(_) => "Integer",
        Value::Float(_) => "Float",
        Value::Decimal(_) => "Decimal",
        Value::String(_) => "String",
        Value::Date(_) => "Date",
        Value::Latest => "Date",
        Value::StrictTime(_) => "StrictTime",
        Value::EnumValue { .. } => "Enum",
        Value::Element(_) => "Element",
        Value::Function(_) => "Function",
        Value::UnitInstance { .. } => "UnitInstance",
        Value::Unit => return None,
        Value::Object(_) | Value::Collection(_) | Value::Map(_) => "Any",
    })
}

fn build_object_inline(class: &str, children: &[DisplayNode], cap: usize) -> String {
    if children.is_empty() {
        return format!("^{class}()");
    }
    let inner: Vec<String> = children
        .iter()
        .map(|c| format!("{} = {}", c.name, c.value))
        .collect();
    let joined = inner.join(", ");
    let candidate = format!("^{class}({joined})");
    if candidate.chars().count() <= cap {
        candidate
    } else {
        format!("^{class}(...)")
    }
}

fn build_collection_inline(children: &[DisplayNode], cap: usize) -> String {
    if children.is_empty() {
        return "[]".to_string();
    }
    let inner: Vec<String> = children.iter().map(|c| c.value.clone()).collect();
    let joined = inner.join(", ");
    let candidate = format!("[{joined}]");
    if candidate.chars().count() <= cap {
        candidate
    } else {
        format!("[... {} items ...]", children.len())
    }
}

fn build_map_inline(children: &[DisplayNode], cap: usize) -> String {
    if children.is_empty() {
        return "{}".to_string();
    }
    let inner: Vec<String> = children
        .iter()
        .map(|c| format!("{} => {}", c.name, c.value))
        .collect();
    let joined = inner.join(", ");
    let candidate = format!("{{{joined}}}");
    if candidate.chars().count() <= cap {
        candidate
    } else {
        format!("{{... {} entries ...}}", children.len())
    }
}

fn read_heap_property(handle: &crate::heap::ObjectHandle, name: &str) -> Vec<Value> {
    handle
        .borrow()
        .get_property_values(name)
        .iter()
        .cloned()
        .collect()
}

/// Read the `name` slot off a Property heap object. The list
/// returned by `properties()` is a list of Property heap entries
/// (Java parity), each carrying a `name` string property.
fn read_prop_name(prop_handle: &crate::heap::ObjectHandle) -> Option<SmolStr> {
    let values = prop_handle.borrow().get_property_values("name");
    values.head().and_then(|v| match v {
        Value::String(s) => Some(s.clone()),
        _ => None,
    })
}

fn collapse_to_value(mut items: Vec<Value>) -> Value {
    match items.len() {
        0 => Value::Unit,
        1 => items.remove(0),
        _ => Value::Collection(Box::new(PVector::from_iter(items))),
    }
}

fn value_key_to_value(key: &ValueKey) -> Value {
    match key {
        ValueKey::Boolean(b) => Value::Boolean(*b),
        ValueKey::Integer(n) => Value::Integer(*n),
        ValueKey::Decimal(d) => Value::Decimal(*d),
        ValueKey::String(s) => Value::String(s.clone()),
        ValueKey::Date(d) => Value::Date(*d),
        ValueKey::StrictTime(t) => Value::StrictTime(*t),
        ValueKey::Object(handle) => Value::Object(handle.clone()),
        ValueKey::ObjectByEqualityKeys { .. } => {
            // Synthetic key — no faithful Value reconstruction. The
            // debugger shows the key name as `<keyed object>` rather
            // than failing.
            Value::String(SmolStr::new("<keyed object>"))
        }
        ValueKey::EnumValue { enum_id, member } => Value::EnumValue {
            enum_id: *enum_id,
            member: member.clone(),
        },
    }
}

/// Best-effort fallback when `toRepresentation` is unavailable (e.g.
/// running the renderer against a model that has not bootstrapped the
/// platform yet). Always something legible — never raw `Debug`.
fn fallback_render(value: &Value) -> String {
    match value {
        Value::Boolean(b) => b.to_string(),
        Value::Integer(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Decimal(d) => d.to_string(),
        Value::String(s) => format!("'{s}'"),
        Value::Unit => "[]".to_string(),
        _ => "<unrendered>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::RuntimeHeap;
    use legend_pure_parser_pure::model::PureModel;
    use std::collections::HashMap;

    /// A minimal `EvalContextTrait` mock that hard-codes `toRepresentation`
    /// to render a few primitive shapes — sufficient to unit-test the
    /// tree-building logic without spinning a full evaluator.
    struct MockCtx<'a> {
        model: &'a PureModel,
        heap: RuntimeHeap,
        extensions: crate::extensions::ExtensionStateStore,
    }

    impl<'a> MockCtx<'a> {
        fn new(model: &'a PureModel) -> Self {
            Self {
                model,
                heap: RuntimeHeap::new(),
                extensions: crate::extensions::ExtensionStateStore::new(),
            }
        }
    }

    impl crate::native::EvalContextTrait for MockCtx<'_> {
        fn evaluate(
            &mut self,
            _spec: &legend_pure_parser_pure::types::ValueSpec,
        ) -> Result<crate::native::Evaluated, crate::error::PureException> {
            unimplemented!("MockCtx::evaluate — not exercised by display tests")
        }

        fn context(&self) -> &crate::context::VariableContext {
            unimplemented!("MockCtx::context — not exercised by display tests")
        }

        fn context_mut(&mut self) -> &mut crate::context::VariableContext {
            unimplemented!("MockCtx::context_mut — not exercised by display tests")
        }

        fn heap(&self) -> &RuntimeHeap {
            &self.heap
        }

        fn heap_mut(&mut self) -> &mut RuntimeHeap {
            &mut self.heap
        }

        fn call_function(
            &mut self,
            _callable: &Value,
            args: &[Value],
        ) -> Result<Value, crate::error::PureException> {
            // The mock only knows toRepresentation — it always
            // returns the fallback rendering. That's enough for
            // tree-shape assertions; full text-fidelity is covered
            // by the integration test.
            Ok(Value::String(SmolStr::new(fallback_render(&args[0]))))
        }

        fn invoke_qualified_property(
            &mut self,
            _receiver: &Value,
            _name: &str,
            _args: &[Value],
        ) -> Result<Option<Value>, crate::error::PureException> {
            Ok(None)
        }

        fn invoke_qualified_property_found(
            &mut self,
            _receiver: &Value,
            _found: &crate::eval::FoundQp,
            _args: &[Value],
        ) -> Result<Value, crate::error::PureException> {
            unreachable!(
                "display::tests::MockCtx::invoke_qualified_property_found is never reached: \
                 the companion lookup-and-invoke returns None for this mock"
            )
        }

        fn model(&self) -> &PureModel {
            self.model
        }

        fn console_output(&mut self, _msg: &str) {}

        fn extensions(&self) -> &crate::extensions::ExtensionStateStore {
            &self.extensions
        }
    }

    #[test]
    fn integer_renders_as_leaf() {
        let model = PureModel::new();
        let mut ctx = MockCtx::new(&model);
        let bindings: HashMap<SmolStr, Value> =
            std::iter::once((SmolStr::new("x"), Value::Integer(42))).collect();
        let tree = render_locals(&mut ctx, bindings.iter(), DisplayConfig::default());
        let nodes = tree.nodes.get(&tree.root).expect("root list");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "x");
        assert_eq!(nodes[0].value, "42");
        assert_eq!(nodes[0].r#type.as_deref(), Some("Integer[1]"));
        assert_eq!(nodes[0].child_ref, 0);
    }

    #[test]
    fn empty_collection_renders_with_zero_child_ref() {
        let model = PureModel::new();
        let mut ctx = MockCtx::new(&model);
        let empty: Value = Value::Collection(Box::new(PVector::new()));
        let bindings: HashMap<SmolStr, Value> =
            std::iter::once((SmolStr::new("xs"), empty)).collect();
        let tree = render_locals(&mut ctx, bindings.iter(), DisplayConfig::default());
        let nodes = tree.nodes.get(&tree.root).expect("root list");
        assert_eq!(nodes[0].value, "[]");
        assert_eq!(nodes[0].child_ref, 0);
    }

    #[test]
    fn collection_of_ints_emits_indexed_children() {
        let model = PureModel::new();
        let mut ctx = MockCtx::new(&model);
        let pv = PVector::from_iter([Value::Integer(1), Value::Integer(2), Value::Integer(3)]);
        let bindings: HashMap<SmolStr, Value> =
            std::iter::once((SmolStr::new("xs"), Value::Collection(Box::new(pv)))).collect();
        let tree = render_locals(&mut ctx, bindings.iter(), DisplayConfig::default());
        let root = tree.nodes.get(&tree.root).expect("root");
        assert_eq!(root[0].indexed_count, Some(3));
        assert!(root[0].child_ref > 0);
        let kids = tree.nodes.get(&root[0].child_ref).expect("kids");
        assert_eq!(kids.len(), 3);
        assert_eq!(kids[0].name, "[0]");
        assert_eq!(kids[0].value, "1");
        assert_eq!(kids[2].value, "3");
    }

    #[test]
    fn depth_cap_collapses_inline() {
        let model = PureModel::new();
        let mut ctx = MockCtx::new(&model);
        let cfg = DisplayConfig {
            depth_cap: 0,
            ..Default::default()
        };
        let bindings: HashMap<SmolStr, Value> =
            std::iter::once((SmolStr::new("x"), Value::Integer(1))).collect();
        let tree = render_locals(&mut ctx, bindings.iter(), cfg);
        let root = tree.nodes.get(&tree.root).expect("root");
        assert_eq!(root[0].value, "<depth limit>");
    }

    #[test]
    fn banned_tokens_do_not_appear_in_any_value() {
        let model = PureModel::new();
        let mut ctx = MockCtx::new(&model);
        let bindings: HashMap<SmolStr, Value> = [
            (SmolStr::new("a"), Value::Integer(42)),
            (SmolStr::new("b"), Value::String(SmolStr::new("hello"))),
            (SmolStr::new("c"), Value::Boolean(true)),
        ]
        .into_iter()
        .collect();
        let tree = render_locals(&mut ctx, bindings.iter(), DisplayConfig::default());
        let banned = [
            "RefCell",
            "Rc<",
            "Dynamic",
            "RuntimeObject",
            "InstanceId",
            "PureDate",
            "Arc<",
            "SmolStr",
        ];
        for nodes in tree.nodes.values() {
            for node in nodes {
                for tok in banned {
                    assert!(
                        !node.value.contains(tok),
                        "banned token {tok:?} in node value {:?}",
                        node.value
                    );
                }
            }
        }
    }
}

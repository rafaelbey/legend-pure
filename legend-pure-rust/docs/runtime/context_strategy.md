# Variable Context Strategy: Design Decision

## Problem

The `VariableContext` needs to support lexical scoping for Pure expression evaluation:
`push_scope` / `set` / `get` / `pop_scope`. The critical question is how to
implement `get()` efficiently, since every `$variable` reference in the
interpreter calls it.

## Strategies Evaluated

### 1. Scope Stack (`Vec<HashMap>`)

Each scope is a separate `HashMap`. Lookup walks scopes in reverse until found.

```rust
struct ScopeStack {
    scopes: Vec<HashMap<SmolStr, Value>>,
}

impl ScopeStack {
    fn get(&self, name: &str) -> Option<&Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(v) = scope.get(name) {
                return Some(v);
            }
        }
        None
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn set(&mut self, name: SmolStr, value: Value) {
        self.scopes.last_mut().unwrap().insert(name, value);
    }
}
```

### 2. Flat Map + Undo Log (chosen)

Single `HashMap` for all visible variables. An undo log records what to
restore when a scope is popped.

```rust
enum UndoEntry {
    Remove(SmolStr),           // Variable was new — remove on pop
    Restore(SmolStr, Value),   // Variable was shadowed — restore old value on pop
}

struct FlatUndo {
    vars: HashMap<SmolStr, Value>,
    undo_stack: Vec<Vec<UndoEntry>>,
}

impl FlatUndo {
    fn get(&self, name: &str) -> Option<&Value> {
        self.vars.get(name)  // Always 1 lookup
    }

    fn set(&mut self, name: SmolStr, value: Value) {
        let frame = self.undo_stack.last_mut().unwrap();
        if let Some(old) = self.vars.get(&name).cloned() {
            frame.push(UndoEntry::Restore(name.clone(), old));
        } else {
            frame.push(UndoEntry::Remove(name.clone()));
        }
        self.vars.insert(name, value);
    }

    fn push_scope(&mut self) {
        self.undo_stack.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        let entries = self.undo_stack.pop().unwrap();
        for entry in entries.into_iter().rev() {
            match entry {
                UndoEntry::Remove(name) => { self.vars.remove(&name); }
                UndoEntry::Restore(name, old) => { self.vars.insert(name, old); }
            }
        }
    }
}
```

### 3. HAMT Snapshot Stack (`Vec<im_rc::HashMap>`)

Each scope is a full HAMT snapshot. `push_scope` clones the current HAMT
(O(1) via structural sharing). `pop_scope` just pops.

```rust
struct HamtSnap {
    scopes: Vec<im_rc::HashMap<SmolStr, Value>>,
}

impl HamtSnap {
    fn get(&self, name: &str) -> Option<&Value> {
        self.scopes.last()?.get(name)  // 1 HAMT lookup
    }

    fn push_scope(&mut self) {
        let snapshot = self.scopes.last().cloned().unwrap_or_default();
        self.scopes.push(snapshot);  // O(1) structural sharing
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();  // Previous snapshot intact
    }

    fn set(&mut self, name: SmolStr, value: Value) {
        self.scopes.last_mut().unwrap().insert(name, value);
    }
}
```

## Benchmark Results

Benchmarked with `criterion` across variable counts (10, 50, 100)
and scope depths (1, 5, 10, 20). Apple Silicon Mac.

### `get()` — Variable Lookup (the hot path)

Outer variable lookup (worst case for scope stack — variable in outermost scope):

| Depth | `ScopeStack` | `FlatUndo` | `HamtSnap` |
|-------|-------------|-----------|-----------|
| 1     | 8.9 ns      | 9.0 ns    | 8.1 ns    |
| 5     | 30.1 ns     | 8.9 ns    | 8.0 ns    |
| 10    | 73.2 ns     | 9.0 ns    | 8.0 ns    |
| 20    | 77.3 ns     | 8.9 ns    | 8.1 ns    |

**Scope stack degrades linearly with depth.** Both flat map and HAMT stay
constant at ~8-9ns regardless of depth.

### Set + Get + Push + Pop Cycle (realistic interpreter loop)

Full lifecycle: push N scopes, set K vars each, lookup all, pop all.

| Depth × Vars/scope | `ScopeStack` | `FlatUndo` | `HamtSnap` |
|---------------------|-------------|-----------|-----------|
| 5 × 2               | 496 ns      | 719 ns    | 1,378 ns  |
| 5 × 5               | 1,329 ns    | 1,816 ns  | 3,109 ns  |
| 10 × 2              | 1,465 ns    | 1,554 ns  | 3,767 ns  |
| 10 × 5              | 3,820 ns    | 3,594 ns  | 8,465 ns  |

HAMT pays ~2x more per `set` due to tree node allocation.

### Push/Pop Only (scope management overhead)

| Parent vars | `ScopeStack` | `FlatUndo` | `HamtSnap` |
|-------------|-------------|-----------|-----------|
| 10          | 36 ns       | 52 ns     | 174 ns    |
| 50          | 37 ns       | 53 ns     | 257 ns    |

## Decision: Flat Map + Undo Log

**Rationale:**

- `get()` is the hottest path (every `$variable` reference). FlatUndo matches
  HAMT at ~9ns while beating scope stack by 9-14x at depth 10-20.
- `set()` and push/pop are competitive with scope stack and 2x faster than HAMT.
- The undo log is ~15 lines of code — trivial to maintain and reason about.
- No extra dependencies needed (uses `std::collections::HashMap`).
- The `undoredo` crate was investigated and rejected — it's designed for
  user-facing undo/redo with commit points and a bistack, not scope management.

### Recreating the Benchmark

To re-run these benchmarks, create a `benches/context_strategies.rs` file
with the three structs above, use `criterion::Criterion` benchmark groups,
and vary `depth` (1, 5, 10, 20) and `total_vars` (10, 50, 100). Benchmark:

1. `get()` for outer and inner variables at each depth
2. Full set+get+push+pop cycles
3. Push/pop with 1 variable set per scope

Register with `[[bench]] name = "context_strategies" harness = false` in
`crates/runtime/Cargo.toml`.

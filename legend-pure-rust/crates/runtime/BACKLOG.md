# Runtime Crate — Backlog

Items are tagged **P0** (blocking) → **P3** (nice-to-have).

---

## Native-function organisation

### [P2] One file per native
Each native function lives in its own `.rs` file alongside its unit tests.
Current layout bundles all natives for a domain (e.g., every string native) into
a single file. Splitting gives a clear 1:1 mapping and keeps each file small.

Proposed layout:
```
native/
  string/
    mod.rs        ← register() / distributed-slice iteration
    plus.rs       ← StringPlus + tests
    length.rs
    format.rs
    …
  comparison/
    mod.rs
    eq.rs
    equal.rs
    less_than.rs
    …
  lang/
    mod.rs
    let_function.rs
    if.rs
    eval.rs
    …
```

### [P2] Distributed-slice self-registration (`linkme`)
Replace per-module `register()` calls with `#[linkme::distributed_slice]` so each
native file self-registers. Adding a new native only requires creating its file.

```rust
// native/mod.rs
#[linkme::distributed_slice]
pub static NATIVES: [(&'static str, &'static dyn NativeFunction)] = [..];

// native/string/format.rs
#[linkme::distributed_slice(crate::native::NATIVES)]
static _FORMAT: (&str, &dyn NativeFunction) =
    ("format_String_1__Any_MANY__String_1_", &Format);
```

`linkme` 0.3+ is compatible with `#![forbid(unsafe_code)]`.
`NativeRegistry::build()` just iterates `NATIVES`.

### [P2] Standard registry built into `Evaluator`
`Evaluator::new` currently requires the caller to build and pass a `NativeRegistry`.
The standard registry (all built-in natives) should be the default; callers that need
to extend or override it can supply their own.

```rust
// today
let registry = NativeRegistry::build();
let mut eval = Evaluator::new(&model, &registry);

// target
let mut eval = Evaluator::new(&model);                        // uses built-in registry
let mut eval = Evaluator::with_registry(&model, registry);   // custom / extended
```

Depends on the distributed-slice item above, since `Evaluator::new` needs a
complete list of natives without manually enumerating them.

---

## PCT — deferred failures (Phase 9)

Tracking the long tail of platform PCT tests that don't pass yet, with the
structural reason and what would unblock each. PCT broad-canary baseline:
**448 PASS of 465 discovered (96.3% pass)**, surveyor at **246/0/0**. Update
this list when items land.

Two buckets: **Excluded** (in `crates/runtime/tests/pct_rust_port.json`,
count as PASS via the exclusion mechanism) and **Tracked** (still FAIL/ERROR
— fix or accept).

### Excluded — intentional

`testAdjust*BigNumber` (5 tests) — assert results years outside `i16`
(PureDate's year field via `jiff::civil::DateTime`). Java Pure carries year
as `i32`. The Rust port's smaller-but-correct trade is to reject extreme
inputs cleanly.

| Test | Reason |
|------|--------|
| `date::tests::testAdjustByMonthsBigNumber`  | year `800002016` overflows i16 |
| `date::tests::testAdjustByWeeksBigNumber`   | year `236611261`  overflows i16 |
| `date::tests::testAdjustByDaysBigNumber`    | year `33803336`   overflows i16 |
| `date::tests::testAdjustByHoursBigNumber`   | year `1410404`    overflows i16 |
| `date::tests::testAdjustByMinutesBigNumber` | year `25489` fits but the *expected* literal `%-21457` overflows the parser's i16 year |

`testLarge{Times,Minus,Plus}` (3 tests) — assert i64-overflowing arithmetic
on Integer literals beyond `i64::MAX` (e.g. `9223372036854775898`,
`18446744073709551614`). The platform marks these
`{test.excludePlatform = 'Java compiled'}` because Java's compiled engine
hits the same wrap-around — same divergence root cause as our Rust port.

| Test | Reason |
|------|--------|
| `math::tests::times::testLargeTimes` | `2 * i64::MAX` overflows; Java compiled excludes too |
| `math::tests::minus::testLargeMinus` | dividend literal exceeds i64; same parity gap |
| `math::tests::plus::testLargePlus`   | sum exceeds i64; same parity gap |

`testDateCompare` (1 test) — asserts on `%10999` (year 10999), outside
jiff's civil::Date `i16` clamp of `-9999..=9999`. Same root cause
category as the BigNumber exclusions: a representational limit, not a
behavioral gap.

| Test | Reason |
|------|--------|
| `lang::tests::compare::testDateCompare` | year `10999` exceeds jiff's `-9999..=9999` |

**Unblocks if** the runtime promotes to a wider type on i64-overflow.
Removing requires either: (a) widening Integer to BigInt, or (b) auto-promoting
to Decimal on detected overflow in `promote_pair`. Both touch the Phase 4
numeric lattice. When done, `apply_exclusion`'s needs-rebase check will flip
these to FAIL with "PCT exclusion needs rebase".

### Tracked — needs investigation or structural work

Grouped by likely fix shape. Each bullet has the test FQN, the failure type
(FAIL = wrong answer; ERROR = exception), and one sentence on what's needed.

#### Date (1 ERROR)
- `date::tests::testDateFromSubSecond` (ERROR) — sub-second datetime
  literal parsing edge (`%2014-01-01T00:00:00.123456789` etc). The
  parser may be truncating fractional digits beyond 9 (jiff nanos
  range). Investigate `parse_subsecond_parts` in `lower.rs`.

#### Lang (1 ERROR)
- `lang::tests::match::testMatchWithMixedReturnType` (ERROR) — `match`
  with branches returning different types, then `->deactivate()` and
  reflect on `$z.genericType.rawType`. Error: "Multiplicity violation:
  expected [1], got 0 values" — the deactivated InstanceValue's
  `genericType` slot isn't populated when match branches return
  diverging types. Needs structural work in match's deactivate path
  to compute the LUB and write it as the genericType.

`testDateCompare` moved to **Excluded — intentional** above (year
10999 outside jiff's `-9999..=9999`).

#### Boolean (1 FAIL)
- `boolean::tests::equality::equal::testEqualNonPrimitive` (FAIL) —
  structural equality on heap objects with non-primitive fields.

#### Collection — empty

(Phase 8 closed the collection cluster: removeDuplicates honours
its optional key/eql Function args, and `Value::Collection.type()`
folds element types through `least_upper_bound_ids` instead of
returning `Any`.)

#### Math (1 FAIL)
- `math::tests::toDecimal::testDoubleToDecimal` (FAIL) — Float→Decimal
  conversion precision; revisit the conversion path.

`testLarge{Times,Minus,Plus}` moved to **Excluded — intentional** above —
same divergence Java compiled excludes.

#### String (2 FAIL)
- `string::tests::toString::testFloatToStringWithExcessTrailingZeros`,
  `toString::testFloatToStringWithPositiveExponent` (2 FAIL) — Float
  Display. Java's `Float.toString` uses specific rules around
  trailing zeros and `E`-notation; Rust's `{f64}` differs.


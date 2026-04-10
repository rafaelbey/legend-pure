# Navigation Path Island Grammar (`#/Type/prop1/prop2#`)

The Navigation Path is a compact syntax for specifying a path through the metamodel type graph. Used in mapping/transformation contexts.

## Research: Both Codebases

### Legend Pure M3 (original)

**Lexer**: `DSL_TEXT: '#' .*? '#'` — captures everything between `#…#` as a single token.

**Dispatch**: `AntlrContextToM3CoreInstance.dsl()` strips the `#` markers, checks if content `startsWith("/")`, and dispatches to the registered `NavigationPath` inline DSL via the `InlineDSLLibrary` service loader mechanism.

**Grammar** (`NavigationParser.g4`):
```antlr
definition:  SEPARATOR genericType (propertyWithParameters)* (name)? EOF
propertyWithParameters:  SEPARATOR VALID_STRING (GROUP_OPEN (parameter (COMMA parameter)*)? GROUP_CLOSE)?
parameter:   scalar | collection
scalar:      atomic | enumStub
enumStub:    VALID_STRING DOT VALID_STRING
atomic:      BOOLEAN | INTEGER | FLOAT | STRING | DATE | LATEST_DATE
name:        EXCLAMATION VALID_STRING
```

**Validation**: `NavigationGraphBuilder` enforces that a path must have **at least one property segment** — an empty path `#/Type#` is a parse error: _"A path must contain at least one navigation"_.

**M3 output**: Creates `meta::pure::metamodel::path::Path` instances with `_start` (GenericType), `_path` (List<PropertyPathElement>), and `_name` (String).

---

### Legend Engine (protocol layer)

**Lexer**: `NAVIGATION_PATH_BLOCK: '#/' (~[#])* '#'` — a specialized single-token capture. Separate from the island grammar (`#{…}#`).

**Dispatch**: In `DomainParseTreeWalker.visitDsl()`, detects `dslNavigationPath()` as a separate branch from `dslExtension()`.

**Grammar** (`NavigationParserGrammar.g4`) — identical structure to M3:
```antlr
definition:  DIVIDE genericType (propertyWithParameters)* (name)? EOF
propertyWithParameters:  DIVIDE VALID_STRING (PAREN_OPEN (parameter ...)? PAREN_CLOSE)?
name:        NOT VALID_STRING
```

**Protocol output**: Creates `ClassInstance { type: "path", value: Path { startType, path: [PropertyPathElement], name } }`.

**Composer rendering**:
```
"#/" + convertPath(startType) + "/" + path.join("/") + "!" + name + "#"
```

Where each property element renders as: `property` or `property(param1, param2)`.

---

### Key Differences Between M3 and Engine

| Aspect | Legend Pure M3 | Legend Engine |
|--------|---------------|---------------|
| Lexer | `#…#` single token, content-dispatched | `#/…#` dedicated token |
| Validation | At least 1 property segment required | Same grammar, validation in walker |
| Type params | `Firm<String>` supported via `genericType` | Same |
| Protocol | M3 `CoreInstance` graph | JSON protocol `ClassInstance("path")` |

---

## Examples from Engine Test Suite

```pure
|$this.employees.lastName->sortBy(#/model::Person/lastName#)->joinStrings('')
|#/Person/nameWithPrefixAndSuffix('a', 'b')#
|#/Person/nameWithPrefixAndSuffix('a', ['a', 'b'])#
|#/Person/nameWithPrefixAndSuffix([], ['a', 'b'])#
```

From M3 tests:
```pure
print(#/Firm<Any>/employees/address#, 2)
```

---

## Proposed Changes

### Lexer (`crates/lexer`)

#### [MODIFY] [token.rs](file:///Users/cocobey73/Projects/legend-engine/legend-pure-parser/crates/lexer/src/token.rs)

Add two new token kinds:

| Token | Pattern | Description |
|-------|---------|-------------|
| `HashSlash` | `#/` | Opens a navigation path |
| `Hash` | `#` | Closes a navigation path (standalone `#`)|

#### [MODIFY] [lexer.rs](file:///Users/cocobey73/Projects/legend-engine/legend-pure-parser/crates/lexer/src/lexer.rs)

Insert between the `###` and `#{` cases:

```rust
// -- Navigation path: #/ --
'#' if self.peek() == Some('/') => {
    self.advance();
    TokenKind::HashSlash
}
```

Add a fallback `'#'` case for the closing hash:

```rust
'#' => TokenKind::Hash,
```

> [!IMPORTANT]
> The order matters: `###` (section header) > `#/` (navigation) > `#{` (island) > `#` (bare hash). The existing `###` check uses `peek()` and `peek_at(1)`, so adding `#/` between that and `#{` is safe.

---

### AST (`crates/ast`)

#### [MODIFY] [expression.rs](file:///Users/cocobey73/Projects/legend-engine/legend-pure-parser/crates/ast/src/expression.rs)

Add a new `NavigationPath` expression variant on the `Expression` enum:

```rust
/// Navigation path expression: `#/Type/prop1/prop2!name#`.
NavigationPath(NavigationPath),
```

New structs:

```rust
/// A navigation path through the type graph: `#/StartType/prop1(args)/prop2!alias#`.
///
/// Produces a `meta::pure::metamodel::path::Path` in the M3 metamodel.
/// Must contain at least one property segment.
#[derive(Debug, Clone, PartialEq, PackageableElement)]
pub struct NavigationPath {
    /// The starting type (e.g., `my::Person`, `Firm<String>`).
    pub start_type: PackageableElementPtr,
    /// The property path segments.
    pub path: Vec<PropertyPathElement>,
    /// Optional alias name (e.g., `myAlias` from `!myAlias`).
    pub name: Option<Identifier>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// A single segment in a navigation path: `/property(params)`.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyPathElement {
    /// The property name.
    pub property: Identifier,
    /// Optional parameters — can include scalars, collections, and enum stubs.
    pub parameters: Vec<Expression>,
    /// Source location.
    pub source_info: SourceInfo,
}
```

---

### Parser (`crates/parser`)

#### [MODIFY] [parser.rs](file:///Users/cocobey73/Projects/legend-engine/legend-pure-parser/crates/parser/src/parser.rs)

In `parse_primary_expression`, add:

```rust
TokenKind::HashSlash => {
    self.cursor.advance();
    self.parse_navigation_path(si)
}
```

New `parse_navigation_path` method:

1. Parse the start type as a qualified name (supporting `::` separators)
2. Loop: if next is `/`, consume and parse property name + optional `(params)`
3. If next is `!`, consume and parse alias name
4. Expect closing `Hash` token
5. Return `Expression::NavigationPath(...)`

> [!NOTE]
> Inside the navigation path, `/` is used as a separator (not `Slash` division operator). The parser handles this contextually — between `#/` and `#`, slashes delimit path segments.

---

### Composer (`crates/compose`)

#### [MODIFY] [expression.rs](file:///Users/cocobey73/Projects/legend-engine/legend-pure-parser/crates/compose/src/expression.rs)

Add rendering for `NavigationPath`:

```rust
Expression::NavigationPath(nav) => {
    w.write("#/");
    compose_element_ptr(w, &nav.start_type);
    for element in &nav.path {
        w.write("/");
        w.write(&element.property);
        if !element.parameters.is_empty() {
            w.write("(");
            // render params comma-separated
            w.write(")");
        }
    }
    if let Some(name) = &nav.name {
        w.write("!");
        w.write(name);
    }
    w.write("#");
}
```

---

### Protocol (`crates/protocol`)

#### [MODIFY] [convert.rs](file:///Users/cocobey73/Projects/legend-engine/legend-pure-parser/crates/protocol/src/v1/convert.rs)

Convert `NavigationPath` → `ClassInstance { type: "path", value: { startType, path, name } }`.

#### [MODIFY] [from_protocol.rs](file:///Users/cocobey73/Projects/legend-engine/legend-pure-parser/crates/protocol/src/v1/from_protocol.rs)

Reverse: `ClassInstance("path")` → `Expression::NavigationPath(...)`.

---

## Verification Plan

### Automated Tests
- **Lexer**: `#/my::Person/name#` tokenizes correctly (HashSlash, Identifier, PathSep, Identifier, Slash, Identifier, Hash, Eof)
- **Roundtrip**: Simple path, nested path, with params, with alias, with collections, with type params
- **Snapshot**: Verify AST structure for navigation paths
- **Validation**: Empty path `#/Type#` should be a parse error (matching M3 behavior)

### Edge Cases
- Type parameters: `#/Firm<String>/employees#`
- Enum stubs in params: `#/Person/status(Status.Active)#`
- Collection params: `#/Person/func(['a', 'b'])#`
- Empty collection params: `#/Person/func([])#`
- Multiple params: `#/Person/func('a', 'b')#`

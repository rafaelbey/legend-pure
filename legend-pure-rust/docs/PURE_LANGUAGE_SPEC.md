# Pure Language — Technical Specification

Derived from the Rust parser (`crates/parser/`) and compiler (`crates/pure/`).
This is a **formal technical specification** for implementors, not a user guide.

For the user-facing language reference, see `/docs/reference/pure-language-reference.md`.

---

## 1. Source File Structure

A Pure source file is a sequence of **sections**, each introduced by a
`###SectionName` header on its own line. Sections without a header default
to `###Pure`.

```ebnf
SourceFile     = Section* EOF
Section        = SectionHeader? ImportStatement* Element*
SectionHeader  = '###' Identifier NEWLINE
ImportStatement = 'import' PackagePath '::*' ';'
```

The parser supports pluggable section grammars via the `SectionPlugin` trait.
The built-in grammar is `###Pure`, documented here.

---

## 2. Lexical Grammar

### Tokens

| Category | Examples |
|----------|---------|
| Keywords | `Class`, `Enum`, `Association`, `Profile`, `Measure`, `Primitive`, `function`, `native`, `let`, `import`, `extends`, `true`, `false` |
| Identifiers | `[a-zA-Z_][a-zA-Z0-9_]*` |
| Integer literals | `42`, `-7`, `0` |
| Float literals | `3.14`, `-0.5`, `1.0` |
| Decimal literals | `3.14d`, `3.14D` |
| String literals | `'hello'`, `'it\'s'` — single-quoted, escaped with `\` |
| Date literals | `%2024-01-15`, `%2024-01-15T10:30:00`, `%2024-01-15T10:30:00+0000` |
| Time literals | `%10:30:00` |
| Operators | `+`, `-`, `*`, `/`, `==`, `!=`, `<`, `<=`, `>`, `>=`, `&&`, `||`, `!`, `->`, `.` |
| Bitwise operators | `&&&`, `|||`, `^^^`, `<<<`, `>>>`, `~~~` |
| Delimiters | `(`, `)`, `{`, `}`, `[`, `]`, `<`, `>`, `:`, `;`, `,`, `|`, `^`, `$`, `@`, `~`, `#` |
| Comments | `// single-line`, `/* multi-line */` |

### String Escaping

| Escape | Meaning |
|--------|---------|
| `\'` | Single quote |
| `\\` | Backslash |
| `\n` | Newline |
| `\t` | Tab |
| `\r` | Carriage return |

### Reserved Identifiers

The following are keywords and cannot be used as element names:
`Class`, `Enum`, `Association`, `Profile`, `Measure`, `Primitive`,
`function`, `native`, `let`, `import`, `extends`, `true`, `false`.

---

## 3. Top-Level Elements

```ebnf
Element = Annotations? ( ClassDef | EnumDef | FunctionDef | NativeFunctionDef
                        | ProfileDef | AssociationDef | MeasureDef | PrimitiveDef )

Annotations = Stereotypes? TaggedValues?
Stereotypes = '<<' StereotypeRef (',' StereotypeRef)* '>>'
StereotypeRef = QualifiedName '.' Identifier
TaggedValues = '{' TaggedValue (',' TaggedValue)* '}'
TaggedValue  = QualifiedName '.' Identifier '=' STRING
```

### 3.1 Class

```ebnf
ClassDef = 'Class' Annotations? QualifiedName TypeParams? TypeVariableParams?
           ('extends' SuperTypeList)?
           Constraints?
           '{' ClassBody '}'

TypeParams          = '<' Identifier (',' Identifier)* ('|' Identifier (',' Identifier)*)? '>'
TypeVariableParams  = '(' TypeVarParam (',' TypeVarParam)* ')'
TypeVarParam        = Identifier ':' TypeReference '[' Multiplicity ']'
SuperTypeList       = TypeReference (',' TypeReference)*
Constraints         = '[' Constraint (',' Constraint)* ']'
ClassBody           = (Property | QualifiedProperty)*
```

#### 3.1.1 Property

```ebnf
Property = Annotations? Aggregation? Identifier ':' TypeSpec '[' Multiplicity ']'
           ('=' DefaultValue)? ';'

Aggregation = '(' ('none' | 'shared' | 'composite') ')'
TypeSpec    = TypeReference | UnitReference | RelationType | FunctionType
```

#### 3.1.2 Qualified Property (Derived)

```ebnf
QualifiedProperty = Annotations? Identifier '(' Parameters ')' '{' ExpressionList '}'
                    ':' TypeSpec '[' Multiplicity ']' ';'
```

#### 3.1.3 Constraint

```ebnf
Constraint = Expression
           | Identifier ':' Expression
           | Identifier '(' ConstraintBody ')'

ConstraintBody = 'enforcement' ':' Identifier ','
                 'externalId' ':' STRING ','
                 'message' ':' Expression ','
                 'function' ':' Expression
```

### 3.2 Enumeration

```ebnf
EnumDef = 'Enum' Annotations? QualifiedName '{' EnumValues '}'
EnumValues = EnumValue (',' EnumValue)* ','?
EnumValue  = Annotations? Identifier
```

### 3.3 Function

```ebnf
FunctionDef = 'function' Annotations? QualifiedName TypeParams?
              '(' Parameters ')' ':' TypeSpec '[' Multiplicity ']'
              '{' ExpressionList '}'
              FunctionTests?

Parameters = (Parameter (',' Parameter)*)?
Parameter  = Identifier (':' TypeReference '[' Multiplicity ']')?

FunctionTests = '{' FunctionTest* '}'
```

### 3.4 Native Function

```ebnf
NativeFunctionDef = 'native' 'function' Annotations? QualifiedName TypeParams?
                    '(' Parameters ')' ':' TypeSpec '[' Multiplicity ']' ';'
```

### 3.5 Profile

```ebnf
ProfileDef = 'Profile' Annotations? QualifiedName
             '{' ProfileBody '}'

ProfileBody = ('stereotypes' ':' '[' IdentifierList ']' ';')?
              ('tags' ':' '[' IdentifierList ']' ';')?
```

### 3.6 Association

```ebnf
AssociationDef = 'Association' Annotations? QualifiedName
                 '{' (Property | QualifiedProperty)* '}'
```

### 3.7 Measure

```ebnf
MeasureDef = 'Measure' Annotations? QualifiedName
             '{' CanonicalUnit? NonCanonicalUnit* '}'

CanonicalUnit    = '*' Identifier (':' ConversionDef)?  ';'
NonCanonicalUnit = Identifier ':' ConversionDef ';'
ConversionDef    = Identifier '->' Expression
```

### 3.8 Primitive

```ebnf
PrimitiveDef = 'Primitive' Annotations? QualifiedName TypeVariableParams?
               'extends' TypeReference
```

---

## 4. Type System

### 4.1 Type References

```ebnf
TypeReference = Package? Identifier TypeArguments? TypeVariableValues?
TypeArguments = '<' TypeReference (',' TypeReference)* MultArgs? '>'
MultArgs      = '|' MultArg (',' MultArg)*
MultArg       = Identifier | Multiplicity
TypeVariableValues = '(' TypeVarValue (',' TypeVarValue)* ')'
TypeVarValue  = INTEGER | STRING
```

### 4.2 TypeSpec Variants

```ebnf
TypeSpec = TypeReference
         | UnitReference
         | RelationType
         | FunctionType

UnitReference = TypeReference '~' Identifier
RelationType  = '(' RelationColumn (',' RelationColumn)* ')'
RelationColumn = Identifier ':' TypeReference ('[' Multiplicity ']')?
FunctionType  = '{' (FuncTypeParam (',' FuncTypeParam)* )? '->' TypeReference '[' Multiplicity ']' '}'
FuncTypeParam = TypeReference '[' Multiplicity ']'
```

### 4.3 Multiplicity

```ebnf
Multiplicity = '[' MultSpec ']'
MultSpec     = INTEGER                          -- [N]
             | INTEGER '..' INTEGER             -- [N..M]
             | INTEGER '..' '*'                 -- [N..*]
             | '*'                              -- [*]
             | Identifier                       -- [m] (variable)
```

| Notation | Semantics | Named Variant |
|----------|-----------|---------------|
| `[1]` | Exactly one | `PureOne` |
| `[0..1]` | Optional | `ZeroOrOne` |
| `[*]` | Zero or more | `ZeroOrMany` |
| `[1..*]` | One or more | `OneOrMany` |
| `[N..M]` | Range | `Range(N, Some(M))` |
| `[m]` | Variable | `Variable("m")` |

### 4.4 Built-in Type Hierarchy

```
Any
├── Boolean
├── Number
│   ├── Integer
│   ├── Float
│   └── Decimal
├── String
├── Date
│   ├── StrictDate
│   └── DateTime
├── StrictTime
├── (user classes)
└── Nil  ← bottom type (subtype of everything)
```

### 4.5 Variance (covariant / contravariant / invariant)

A class type-parameter slot has one of three variances:

| Variance | Surface syntax | Subtype rule on `Container<X>` |
|----------|----------------|--------------------------------|
| **Invariant** (default) | `Class C<T>` | `Container<X>` and `Container<Y>` are unrelated unless `X == Y` exactly. |
| **Covariant** | `Class C<+T>` | `X <: Y` ⇒ `Container<X> <: Container<Y>`. Output position. |
| **Contravariant** | `Class C<-T>` | `X <: Y` ⇒ `Container<Y> <: Container<X>` (flipped). Input position. |

Pure has TWO surface forms for declaring variance — both compile to
the same `Variance` flag on the class's type-parameter slot, and
behave identically downstream:

1. **Class-level prefix syntax** — `Class C<-T, +U, V>` puts the
   marker on the class declaration. Used by `path.pure`'s
   `Path<-U, V|m>` to mark Path's owner slot contravariant.

2. **Metamodel instance form** — `^TypeParameter{name:'T',
   contravariant:true}` declares variance from inside the M3
   metamodel. m3.pure uses this form for the platform's built-in
   contravariant classes:
   - `Property<U[contravariant], V>` — every reflective property
     access depends on this.
   - `Column<U[contravariant], V>` — relation/Column.
   - `NewPropertyRouteNodeFunctionDefinition<U[contravariant], V>`
     — Path's treepath machinery.

Both forms reach the compiler as
`crate::nodes::class::Variance::Contravariant`.

#### Why Property's contravariant U matters at platform scale

`Property<U, V>` represents a property whose owner type is `U`.
Contravariance says `Property<D_A, V>` is a SUBTYPE of
`Property<Nil, V>` (because `Nil <: D_A` in the contravariant slot
flips), so a Property-of-D_A can be passed wherever a
Property-of-Nil is expected.

The platform's reflective dispatch chains exploit this:

```pure
function getProperty(class: Class<Any>[1], name: String[1]):
    Property<Nil, Any|*>[0..1] { ... }

// Use site (dynamicNew.pure):
D_A->getProperty('a')->toOne()->eval($r)
```

`getProperty` returns `Property<Nil, Any|*>[0..1]` — the most-generic
Property type. Without contravariance, eval'ing that property against
a `D_A`-typed receiver `$r` would freeze `T` to `Nil` from the
structural lift and reject the arg as "expected Nil, got D_A". The
contravariance rule lifts the `Nil` to `Any` (the dual under
contravariance — a property-of-Nil accepts any owner), letting the
chain dispatch.

Three platform files exercise this pattern heavily:

- `dynamicNew.pure` — dynamic instance construction + reflection.
- `eval/eval.pure` — eval-against-property tests.
- `meta/reflect/canReactivateDynamically.pure` — value-spec
  reactivation chains.
- `dsl-mapping/PropertyMappingsImplementation.pure` — mapping reflection.

This is why the Rust port's variance support spans both surface
forms: forgetting contravariance silently breaks the entire
metamodel-reflection chain. The feature is sparsely-documented in
Java's source and easy to miss.

#### Compiler implementation notes

- `Class.type_parameter_variances: Vec<Variance>` is position-aligned
  with `Class.type_parameters` (Vec<SmolStr>). `#[serde(default)]`
  on the field lets old `.purem` blobs deserialize as all-Invariant
  while new builds capture the metadata.
- The m3 parser at `crates/pure/src/m3_parser.rs` reads
  `contravariant: true` / `covariant: true` from
  `^TypeParameter{...}` instance forms (the lexer maps the literal
  `true` to `Token::Ident("true")`).
- The class-level `<-T>` / `<+T>` prefix parsing in
  `crates/parser/src/parser/type_ref.rs` accepts the markers; the
  AST → compiled-Class wiring stamps the resulting `Variance` flags.
- `subtype_view` (the supertype-walk used during structural binding)
  applies the Nil-→-Any lift for contravariant slots when
  substituting a class's type-args into its super-types. This is the
  single hot spot that makes contravariance flow through the
  reflective-dispatch chain without touching every consumer.
- Tests: `crates/pure/tests/variance_tests.rs` has 5 tests covering
  positive contravariance through Property, the negative case
  (invariant user-class rejects the same shape), default-mode Java
  parity, and a wire-level pin that the loaded platform's Property
  carries the contravariant flag.

---

## 5. Expression Grammar

### 5.1 Operator Precedence (lowest → highest)

| Level | Operators | Associativity | AST Node |
|-------|-----------|---------------|----------|
| 1 | `||` | Left | `LogicalExpr(Or)` |
| 2 | `&&` | Left | `LogicalExpr(And)` |
| 3 | `==`, `!=`, `<`, `<=`, `>`, `>=` | Left | `ComparisonExpr` |
| 4 | `+`, `-` | Left | `ArithmeticExpr(Plus/Minus)` |
| 5 | `*`, `/` | Left | `ArithmeticExpr(Times/Divide)` |
| 6 | `!`, `-` (unary), `~~~` | Prefix | `NotExpr`, `UnaryMinusExpr`, `BitwiseNotExpr` |
| 7 | `->`, `.` | Left (postfix) | `ArrowFunction`, `MemberAccess` |
| 8 | primary | — | literals, variables, function calls |

Bitwise operators (`&&&`, `|||`, `^^^`, `<<<`, `>>>`) sit at precedence level 3
(same as comparison). F#-style triple-character operators avoid ambiguity with
existing Pure syntax (`|`, `^`, `<<`, `>>`).

### 5.2 Expressions

```ebnf
Expression     = OrExpr
OrExpr         = AndExpr ('||' AndExpr)*
AndExpr        = CompExpr ('&&' CompExpr)*
CompExpr       = AddExpr (CompOp AddExpr)*
AddExpr        = MultExpr (('+' | '-') MultExpr)*
MultExpr       = UnaryExpr (('*' | '/') UnaryExpr)*
UnaryExpr      = '!' UnaryExpr | '-' UnaryExpr | '~~~' UnaryExpr | PostfixExpr
PostfixExpr    = Primary (DotAccess | ArrowAccess)*

DotAccess      = '.' Identifier ( '(' ArgList ')' )?
ArrowAccess    = '->' QualifiedName '(' ArgList ')'

Primary        = Literal
               | '$' Identifier                     -- Variable
               | '@' TypeSpec                       -- Type reference
               | '^' QualifiedName TypeArgs? '(' KeyValues ')'  -- New instance
               | '^' '$' Identifier '(' KeyValues ')'           -- Copy
               | '{' LambdaParams '|' ExpressionList '}'        -- Block lambda
               | BareLambda                         -- x | expr (in arg context)
               | 'let' Identifier '=' Expression    -- Let binding
               | '[' ExpressionList ']'             -- Collection / slice
               | '~' ColumnSpec                     -- Column builder
               | '#' IslandGrammar                  -- Island expression
               | '(' Expression ')'                 -- Grouping
               | QualifiedName '(' ArgList ')'      -- Function call
               | QualifiedName                      -- Element reference
               | INTEGER UnitPath                   -- Unit instance: 5 Measure~Unit
```

### 5.3 Lambda Expressions

Two syntactic forms:

```ebnf
-- Block lambda (braced)
BlockLambda = '{' LambdaParams '|' ExpressionList '}'

-- Bare lambda (in argument position only)
BareLambda  = LambdaParams '|' Expression

LambdaParams = LambdaParam (',' LambdaParam)*
LambdaParam  = Identifier (':' TypeReference '[' Multiplicity ']')?
```

Parameters without type annotations have `type_ref = None`, `multiplicity = None`.
The compiler infers types from the enclosing call context.

### 5.4 Expression Lists and Semicolons

```ebnf
ExpressionList = Expression                         -- single (NO semicolon)
               | Expression ';' ExpressionList      -- multiple (ALL semicoloned)
```

- **Single expression:** no semicolon
- **Multiple expressions:** every expression (including last) terminated with `;`
- Empty expressions (`;;`) are parse errors

### 5.5 Collection and Slice

```ebnf
CollectionExpr = '[' ']'                            -- empty collection
               | '[' Expression (',' Expression)* ']'  -- collection
               | '[' Start? ':' Stop (':' Step)? ']'   -- slice expression
```

No trailing commas. No empty slots.

### 5.6 New Instance and Copy

```ebnf
NewInstance = '^' QualifiedName TypeArgs? TypeVarValues? '(' KeyValuePairs? ')'
Copy       = '^' '$' Identifier '(' KeyValuePairs? ')'

KeyValuePairs = KeyValuePair (',' KeyValuePair)*
KeyValuePair  = Identifier '=' Expression
```

### 5.7 Unit Instance

```ebnf
UnitInstance = NumericLiteral QualifiedName '~' Identifier
```

Example: `5 RomanLength~Pes`, `10.5D pkg::Mass~Kilogram`

### 5.8 Island Grammar (Graph Fetch Trees)

```ebnf
IslandExpr = '#' Tag? '{' IslandContent '}#'

-- Graph fetch (tag = "")
GraphFetch = '#{' RootClass '{' PropertyTree* SubTypeTree* '}' '}#'
PropertyTree = Alias? Property Params? SubType? ('{' PropertyTree* SubTypeTree* '}')?
SubTypeTree  = '->subType' '(' '@' TypeRef ')' '{' PropertyTree* SubTypeTree* '}'
Alias = STRING ':'
Params = '(' ExpressionList ')'
SubType = '->subType' '(' '@' TypeRef ')'
```

### 5.9 Column Builder (TDS)

```ebnf
ColumnBuilder = '~' ColumnSpec
              | '~' '[' ColumnSpec (',' ColumnSpec)* ']'

ColumnSpec = Annotations? Identifier (':' ColumnTypeSpec)? ('::' Expression)?
ColumnTypeSpec = TypeReference ('[' Multiplicity ']')?
               | LambdaParam '|' Expression
```

---

## 6. Naming Conventions

### 6.1 Qualified Names

```ebnf
QualifiedName = PackagePath? Identifier
PackagePath   = Identifier ('::' Identifier)* '::'
```

### 6.2 Function Name Mangling

Functions are identified by mangled fully-qualified names (matching Java's
`ConcreteFunctionDefinitionNameProcessor`):

```
funcName_ParamType_MultSig__ParamType_MultSig__ReturnType_MultSig_
```

| Multiplicity | Mangled |
|-------------|---------|
| `[1]` | `_1_` |
| `[0..1]` | `_$0_1$_` |
| `[*]` | `_MANY_` |
| `[1..*]` | `_$1_MANY$_` |
| `[m]` (variable) | `_m_` |
| `[N..M]` | `_$N_M$_` |

No-parameter functions: `funcName__ReturnType_MultSig_` (extra `_`).

---

## 7. Compiler Semantics

### 7.1 Compilation Pipeline

```
Pass 1 → Pass 1.5 → Pass 2a → Pass 2b → Freeze → Pass 3
```

| Pass | Name | Action |
|------|------|--------|
| 1 | Declaration | Assign `ElementId`s, allocate shells, build package tree |
| 1.5 | Topological Sort | DAG from supertype edges, Kahn's algorithm, detect cycles |
| 2a | Resolve Signatures | Hydrate parameter types, multiplicities, return types |
| 2b | Resolve Bodies | Compile expression bodies; all signatures available for dispatch |
| Freeze | Rebuild Indexes | Compute specialization edges, association-injected properties |
| 3 | Validation | Read-only semantic checks (8 validators) |

### 7.2 Function Overload Resolution (Dispatch)

Five-phase narrowing:

1. **Compatibility Filter** — remove overloads incompatible with argument types
2. **Specificity Scoring** — exact type (+3), subtype (+1), exact mult (+4)
3. **Parameter Domination** — eliminate overloads whose params are all supertypes of another
4. **Type Distance** — shortest hop count in type hierarchy
5. **Declaration Order** — first-declared wins (matches Java behavior)

Arguments are **lowered before dispatch** (the "Lower First" pattern).

### 7.3 Type Inference

Types inferred from lowered `ExprKind`:

| Expression | Inferred Type |
|-----------|---------------|
| Integer literal | `Integer` |
| Float literal | `Float` |
| Decimal literal | `Decimal` |
| String literal | `String` |
| Boolean literal | `Boolean` |
| StrictDate literal | `StrictDate` |
| DateTime literal | `DateTime` |
| StrictTime literal | `StrictTime` |
| Enum value | The `Enumeration` element |
| Function call | Return type of resolved function |
| Variable | Looked up from `variable_types` |
| Lambda, Collection | Unknown (`None`) |

### 7.4 Multiplicity Compatibility

An argument multiplicity `[a..b]` is compatible with parameter `[p..q]` when:
- `a >= p` (arg lower ≥ param lower)
- `b <= q` (arg upper ≤ param upper)

### 7.5 Subtype Checking

`is_subtype(child, parent)` walks the `super_types` chain on Class and
PrimitiveType elements. The chain `Integer → Number → Any` makes
`Integer` a subtype of `Number` and `Any`.

---

## 8. Semantic Validations

Validations deferred from parser to compiler:

| ID | Validation | Parser Behavior | Compiler Action |
|----|-----------|----------------|-----------------|
| SV-001 | Function test name mismatch | Accepts | Error |
| SV-002 | `->subType` not at root of graph fetch | Accepts at any depth | Error |
| SV-003 | Lambda parameter type inference | `type_ref = None` | Infer types |

---

## 9. Standard Library Functions

All standard library functions live under `meta::pure::functions::`.

### Core Function Signatures

```pure
// Control flow
if<T,m>(test: Boolean[1], valid: Function<{->T[m]}>[1], invalid: Function<{->T[m]}>[1]): T[m]
match<T,m,n>(var: Any[*], functions: Function<{Nil[n]->T[m]}>[1..*]): T[m]
letFunction<T,m>(left: String[1], right: T[m]): T[m]

// Type operations
cast<T|m>(source: Any[m], object: T[1]): T[m]
instanceOf(instance: Any[1], type: Type[1]): Boolean[1]
new<T>(class: Class<T>[1], name: String[1], keyExpressions: KeyExpression[*]): T[1]

// Collection core
filter<T>(set: T[*], func: Function<{T[1]->Boolean[1]}>[1]): T[*]
map<T,V|m>(set: T[*], func: Function<{T[1]->V[m]}>[1]): V[*]
fold<T,V|m>(set: T[*], func: Function<{T[1],V[m]->V[m]}>[1], init: V[m]): V[m]
size(set: Any[*]): Integer[1]
toOne<T>(collection: T[*]): T[1]
toOneMany<T>(collection: T[*]): T[1..*]

// Arithmetic (all map to operators)
plus(left: Number[1], right: Number[1]): Number[1]
minus(left: Number[1], right: Number[1]): Number[1]
times(left: Number[1], right: Number[1]): Number[1]
divide(left: Number[1], right: Number[1]): Float[1]

// String
plus(strings: String[*]): String[1]           -- concatenation
toString(value: Any[1]): String[1]
format(template: String[1], args: Any[*]): String[1]
```

---

## Appendix A: Grammar Differences from Java Parser

| Feature | Java (ANTLR) | Rust |
|---------|-------------|------|
| Parser type | Generated (ANTLR4) | Hand-written recursive descent |
| Semantic checks in parser | Yes (some) | No — strictly syntactic |
| Lambda param inference | Fabricates `Any[*]` | `None` (compiler infers) |
| Untyped lambda params | `Parameter.type` = `Any` | `Parameter.type_ref` = `None` |
| `Expression::Group` | Transparent (no AST node) | Preserved (for roundtrip) |
| Bitwise operators | Not supported | `&&&`, `|||`, `^^^`, `<<<`, `>>>`, `~~~` |
| Relation types | External grammar | First-class `TypeSpec::Relation` |
| Function types in TypeRef | Encoded via protocol | First-class `TypeSpec::Function` |

## Appendix B: Complete Expression Node Types

| AST Node | Syntax | Example |
|----------|--------|---------|
| `IntegerLiteral` | `[0-9]+` | `42` |
| `FloatLiteral` | `[0-9]+\.[0-9]+` | `3.14` |
| `DecimalLiteral` | `[0-9]+\.[0-9]+[dD]` | `3.14D` |
| `StringLiteral` | `'...'` | `'hello'` |
| `BooleanLiteral` | `true` / `false` | `true` |
| `StrictDateLiteral` | `%YYYY-MM-DD` | `%2024-01-15` |
| `DateTimeLiteral` | `%YYYY-MM-DDThh:mm:ss` | `%2024-01-15T10:30:00` |
| `StrictTimeLiteral` | `%hh:mm:ss` | `%10:30:00` |
| `Variable` | `$name` | `$x` |
| `ArithmeticExpr` | `a op b` | `$x + 1` |
| `ComparisonExpr` | `a op b` | `$x > 0` |
| `LogicalExpr` | `a op b` | `$x && $y` |
| `BitwiseExpr` | `a op b` | `$x &&& $y` |
| `NotExpr` | `!a` | `!$flag` |
| `UnaryMinusExpr` | `-a` | `-$x` |
| `BitwiseNotExpr` | `~~~a` | `~~~$bits` |
| `FunctionApplication` | `f(args)` | `greet('Alice')` |
| `ArrowFunction` | `x->f(args)` | `$xs->filter(...)` |
| `SimpleMemberAccess` | `x.y` | `$p.name` |
| `QualifiedMemberAccess` | `x.y(args)` | `$p.derived('a')` |
| `PackageableElementRef` | `Name` | `String`, `my::Enum` |
| `TypeReferenceExpr` | `@Type` | `@Person` |
| `Lambda` | `{p \| body}` | `{x \| $x + 1}` |
| `LetExpr` | `let x = e` | `let x = 42` |
| `CollectionExpr` | `[a, b]` | `[1, 2, 3]` |
| `SliceExpr` | `[s:e:step]` | `[0:5]` |
| `NewInstanceExpr` | `^C(k=v)` | `^Person(name='A')` |
| `CopyExpr` | `^$v(k=v)` | `^$p(age=30)` |
| `UnitInstanceExpr` | `n M~U` | `5 Mass~Kg` |
| `ColumnBuilderExpr` | `~col` | `~name` |
| `IslandExpression` | `#tag{...}#` | `#{...}#` |
| `Group` | `(expr)` | `($x + 1)` |

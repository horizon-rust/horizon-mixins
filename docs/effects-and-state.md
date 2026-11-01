# Expression results, after hooks, interior cancellation and state (0.5.0)

Plans use schema 7. Regenerate older plans; the transformer verifies its version,
Rust toolchain, source hashes and original site fingerprints before rewriting.
These are build-time Rust contracts, not Java annotation compatibility.

## Expression-result modification

`MODIFY_EXPRESSION_VALUE` selects a complete original expression by exact syntax.
Whitespace is ignored through Rust token parsing; parentheses and path spelling
remain significant. `require` is the exact number of matching original
expressions in the optional slice, and `ordinal` is zero-based. Each declaration
selects one match. Helpers and generated initializer calls are never matched.

```toml
[[injection]]
id = "result"
target = "game::calculate"
kind = "MODIFY_EXPRESSION_VALUE"
selector = "calculate_bonus(input) > 0"
require = 1
ordinal = 0
helper = "result.rs"
bindings = [{ name = "value", ty = "bool", value = "$value" }]
```

```rust
fn modify(value: bool) -> bool { !value }
```

Exactly one `$value` binding is required; the helper returns that same explicit
type. Matching modifiers form a chain in priority descending, mod ID ascending,
injection ID ascending order. An original expression evaluates once and keeps
its branch/short-circuit behavior. Calls, literals, arithmetic/comparisons,
blocks, if/match values, tuples, arrays, struct construction and casts are
supported runtime value shapes. Paths, fields and indexing are limited to
primitive scalar reads; place expressions, method receivers/base places,
reference construction, assignment, control transfers, const contexts and
unsupported macro/async/closure contexts fail closed. The declared type is a
contract, not inferred type information; staged Cargo verifies it. Owned call
results can be transformed without cloning, and reference-returning helpers
must have ordinary valid Rust lifetime signatures.

## After a call, before storing its result

`AFTER_CALL` uses the same callable selector/count/ordinal syntax as BEFORE_CALL.
An explicit `result` names the original call's owned or unit result type. The
helper returns unit, with optional `$result_ref` and `$result_mut` bindings for
borrowing that result. Ordinary parameter bindings and lexical captures also
work. Rust rejects overlapping mutable borrows.

```toml
[[injection]]
id = "observe"
target = "game::calculate"
kind = "AFTER_CALL"
selector = "calculate_bonus"
require = 1
ordinal = 0
result = "i32"
helper = "observe.rs"
bindings = [{ name = "result", ty = "&mut i32", value = "$result_mut" }]
```

```rust
fn observe(result: &mut i32) { *result += 1; }
```

The call, receiver and arguments evaluate once. Hooks run after a successful
call, before the result is stored or consumed by its enclosing expression.
They do not run if the call panics. Multiple hooks observe previous mutations;
the result is forwarded without Clone/Copy. Noncancellable after hooks support
nested call sites through typed adapters whose original arguments stay outside
new block scopes. Borrowed call results are deliberately rejected
(`E_AFTER_RESULT`); use AFTER_ASSIGN to inspect a stored borrowed result or an
expression modifier with a valid explicit lifetime contract. Discarded owned
results drop before receiver/argument temporaries, including when cancelling.

## After assigning the result

`AFTER_ASSIGN` runs after the complete original statement, including assignment
and destruction of statement temporaries. It requires a whole call initializer
of a plain `let` (optional type annotation/parentheses), or the whole RHS of a
semicolon-terminated simple local/parameter assignment. It rejects destructured
lets, let-else, nested RHS calls, field/index assignments. No `result` declaration or `$result_*` bindings apply here.

```toml
[[injection]]
id = "stored"
target = "game::calculate"
kind = "AFTER_ASSIGN"
selector = "calculate_bonus"
require = 1
ordinal = 0
helper = "stored.rs"
bindings = [{ name = "value", ty = "&mut i32", value = "$local_mut:value:0" }]
```

```rust
fn stored(value: &mut i32) { *value += 10; }
```

For `let mut value = calculate_bonus(input);`, the newly initialized binding is
visible to this hook. Captures resolve against the original post-statement
lexical environment, including shadowing. A mutable capture requires `let mut`.
Hooks can borrow, change or consume the stored binding; Cargo checks subsequent
uses. A RETURN helper's capture must also exist at every added cancellation
exit; captures visible at all exits are supported.

## Interior cancellation

BEFORE_CALL, AFTER_CALL and AFTER_ASSIGN accept `cancellable = true`. Their
helper returns `::core::ops::ControlFlow<TargetResult>`; `Continue(())` continues,
`Break(value)` returns from the target function. This is not just cancellation
of a single operation. Target results must be explicit owned types or unit for
interior cancellation; HEAD retains its previous lifetime contract.

BEFORE_CALL cancels before evaluating the selected receiver/callee or arguments.
AFTER_CALL cancels after the call and after observing/mutating its result.
AFTER_ASSIGN cancels after the result has been stored. The first Break skips
later same-phase hooks and all remaining target body code. RETURN modifiers
also process cancelled values. Rust drops initialized locals and temporaries;
no panic, hidden exception, global flag or duplicated expression is used.

Cancellable AFTER_CALL is limited to whole-expression sites (statements, plain
let initializers, tails), like BEFORE_CALL. AFTER_ASSIGN uses its stricter
statement proof. Arbitrary nested interior cancellation is unsupported because
it can change temporary lifetimes. A nested AFTER_CALL without cancellation
remains supported.

At one call site the phases are BEFORE_CALL → argument modifiers → argument
tuple modifiers → operation wrapping/original call → AFTER_CALL → expression
result modification → variable STORE → AFTER_ASSIGN → eventual RETURN.
Constants and variable LOAD run where their original operands evaluate.
Priorities order hooks within a phase; they do not reorder these phases.
An AFTER_CALL cancellation bypasses later expression/STORE/AFTER_ASSIGN phases.

## Persistent per-instance state

A `[[state]]` declaration adds an owned, private namespaced field to a named,
nongeneric struct. It persists for the lifetime of that instance, independently
of every other instance. State initialization and access happen inside the
owner crate; no cross-crate private access or dependency cycle is introduced.

```toml
[[state]]
id = "history"
target = "game::entity::Entity"
ty = "Vec<i32>"
initial = "::core::default::Default::default()"
# priority = 100  # defaults to mod priority

[[state.constructor]]
target = "game::entity::Entity::new"
selector = "Self"
require = 1
```

For mod ID `example`, this creates private field
`__horizon_mixins_example_history`, getter of the same name returning `&Vec<i32>`,
and `__horizon_mixins_example_history_mut()` returning `&mut Vec<i32>`. Generated
identifiers lowercase IDs and replace hyphens with underscores; collisions fail
planning. The type must be explicit and owned. Its initializer is an expression
compiled in a standalone function in the owner's module, with no constructor
parameters or local captures. An initializer may call existing/support-module
APIs, using ordinary Rust paths. Neither dependencies nor serialization traits
are added to Cargo manifests.

Declare every syntactically resolved struct literal using exact constructor
function IDs, literal path selectors and positive exact counts. All matching
literals for each constructor declaration are planned. Unlike injections,
constructors have no ordinal: each declared literal must remain initialized.
`Self`, simple owner names and explicit crate/self/super paths are resolved;
import aliases and generic paths are not inferred. Missing visible constructors,
duplicate sites, invalid counts and OVERWRITE removing a constructor fail
planning. Unsupported constructor function bodies fail closed. Cargo additionally
catches hidden/aliased literals or patterns that need updating; `check` is required.

New construction evaluates original fields first, then state initializers in
priority/ID order. A struct update `Self { health, ..old }` moves its existing
state through `..old`; it does not reset state. Derived Clone clones state;
Drop runs with normal Rust ownership. Derived Default is allowed only with the
literal initial expression `::core::default::Default::default()` for every added
state, so derived and explicit construction agree. A custom Default impl must
declare its own literals as constructors.

Named structs with repr/layout attributes, unresolved cfg or macros, unknown
derives, generic owners, tuple/unit owners, or wholly public externally
constructible fields are unsupported. Use a private field or a non_exhaustive
struct to keep external construction closed. Exact struct destructuring must
have `..`; the planner diagnoses visible patterns without it. State changes the
layout; it does not implement arbitrary members, trait merging or Java Unique.

For structs already deriving serde Serialize/Deserialize, new fields participate
in their normal derived serialization. Deserialize fields get
`#[serde(default = "<namespaced initializer>")]`, allowing older records to load.
The fixture checks an actual JSON roundtrip and old-data defaults. Types must
already implement required derives; serde must already be a dependency. Renaming
mod/state IDs changes the serialized field name. Other save formats, custom
serialization, schema migrations and game world persistence need explicit mod
code. Instance lifetime support does not automatically connect state to a
world's save system. Removing a mod removes its staged fields; keep original
save data/backups according to that save system's own policy.

## Validation

```sh
cargo fetch --locked --manifest-path fixtures/effects/Cargo.toml
cargo run --locked -p horizon-mixins-cli -- check \
  --workspace fixtures/effects --mods fixtures/effects-mods \
  --out target/effects-game --trust-native-mods
```

The two-mod fixture compiles five runtime tests for once-only evaluation,
short-circuit behavior, hook order, cancellation/RETURN, Drop timing, typed
post-assignment captures, state isolation, clone/update lifecycle and serde.
Engine tests add failure cases, move/type errors, cross-phase signature checks,
stale IR, deterministic source/plan snapshots and unchanged source/mtime checks.
`tools/effects-feasibility.rs` independently compiles the adapter/lifecycle spike
through `tools/feasibility.py`.

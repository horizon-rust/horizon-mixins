# Scoped injections and bridges (0.3.0)

Version 0.3.0 implements lexical local capture, accessor/invoker bridges,
original-source slices, MODIFY_ARGS and MODIFY_VARIABLE. The baseline and
advanced fixtures continue to pass. Plans use schema 7; regenerate older plans.

Run the two-mod fixture containing 25 injections, nine bridges and eight runtime
tests:

```sh
cargo run --locked -p horizon-mixins-cli -- check \
  --workspace fixtures/scoped --mods fixtures/scoped-mods \
  --out target/scoped-game --trust-native-mods
```

Examples below use the fixture's source identities. Each helper is a separate
Rust file relative to its bundle manifest. See the complete
[alpha manifest](../fixtures/scoped-mods/alpha/horizon-mixin.toml),
[beta manifest](../fixtures/scoped-mods/beta/horizon-mixin.toml) and
[original game source](../fixtures/scoped/game/src/lib.rs).

## Local capture

Local captures explicitly select a declaration with `$local:NAME:ORDINAL`,
`$local_ref:NAME:ORDINAL`, or `$local_mut:NAME:ORDINAL`. Ordinals count original
local declarations with that name in AST order, not parameters. Name lookup at
the selected site must still resolve to that exact declaration. The helper
binding supplies an explicit type; annotated local types are checked during
planning, and Cargo checks inferred types, moves and borrows. Plain let bindings
are supported; destructuring/conditional pattern bindings are tracked as scope
barriers and rejected as capture targets. HEAD/OVERWRITE have no original local
context. RETURN capture must be visible at every rewritten exit; cancellation
before declarations cannot supply captured return context. Wrappers still accept
only their operation inputs.

```toml
[[injection]]
id = "capture"
kind = "BEFORE_CALL"
target = "scoped_game::locals"
selector = "tick"
require = 3
ordinal = 0
helper = "capture.rs"
bindings = [
  { name = "value", ty = "&mut i32", value = "$local_mut:value:0" },
  { name = "log", ty = "&mut Vec<&'static str>", value = "&mut *log" },
]
```

```rust
fn capture(value: &mut i32, log: &mut Vec<&'static str>) {
    *value += 10;
    log.push("capture");
}
```

The selector denotes the first original local declaration called `value`, not
the first currently visible binding. If an inner declaration shadows it at the
selected call, planning fails instead of capturing the inner value. A local is
not visible in its own initializer. Mutable capture requires a mutable binding.
`$local` passes a value using ordinary Rust move/Copy rules; reference forms
borrow it. Inferred types must still be stated in the helper binding. Locals
with annotated types are compared syntactically, without alias resolution.

RETURN captures must be visible at every explicit and implicit exit. A function
with cancellable HEAD and captured RETURN context is rejected because HEAD can
exit before those declarations. No local capture is implicit, and no fallback
handler/surrogate is selected. Relevant diagnostics include `E_LOCAL_NOT_FOUND`,
`E_LOCAL_NOT_VISIBLE`, `E_CAPTURE_TYPE`, `E_UNSUPPORTED_LOCAL` and
`E_INVALID_CAPTURE`; actual type, move
and borrow failures are reported as `E_BUILD` with Cargo output.

## Slices

A slice supplies optional `from` and `to` call selectors. Each boundary requires
its own exact global `require` and zero-based `ordinal`. At least one boundary
is required. Bounds are original expression positions, inclusive from/exclusive
to; the injection's own require/ordinal operate inside the slice. Boundaries
must be whole-expression calls in the same lexical block. A single boundary
must be in the function root block. No boundary is matched against generated
code. Invalid/missing/reversed/cross-scope boundaries fail planning.

```toml
[[injection]]
id = "slice-arg"
kind = "MODIFY_ARG"
target = "scoped_game::sliced"
selector = "consume"
require = 1
ordinal = 0
argument = 0
slice = { from = { selector = "begin", require = 1, ordinal = 0 }, to = { selector = "end", require = 1, ordinal = 0 } }
helper = "slice-arg.rs"
bindings = [{ name = "value", ty = "i32", value = "$value" }]
```

Only calls to `consume` between the original `begin` and `end` landmarks count
toward this injection's `require = 1`. Omitting `from` starts at the root block's
start; omitting `to` ends at its end. Slices can restrict BEFORE_CALL, MODIFY_ARG,
MODIFY_ARGS, MODIFY_CONSTANT, MODIFY_VARIABLE and WRAP_OPERATION. They cannot
restrict HEAD, RETURN or OVERWRITE. No bytecode offsets or shifted anchors are
accepted. Boundary errors use `E_SLICE`, `E_SLICE_COUNT`, `E_SLICE_ORDINAL`,
`E_SLICE_SCOPE` and `E_SLICE_ORDER`.

## Joint argument modification

MODIFY_ARGS receives one `$args` binding typed as a concrete tuple, returns that
same tuple, and requires a concrete owned/unit `result` type for the selected
call. All original explicit arguments are evaluated once in order. The receiver
is separate. Individual MODIFY_ARG chains run before tuple chains; tuple chains
run before WRAP_OPERATION. Borrowed method receivers and static function paths
follow the wrapper restrictions. Context parameters and visible locals are
explicit bindings. Borrowed call results, computed/consuming receivers and
unproven two-phase borrow cases are rejected.

```toml
[[injection]]
id = "tuple"
kind = "MODIFY_ARGS"
target = "scoped_game::jointly"
selector = "combine"
require = 1
ordinal = 0
result = "String"
helper = "tuple.rs"
bindings = [
  { name = "args", ty = "(String,i32,Trace)", value = "$args" },
  { name = "bonus", ty = "&i32", value = "$local_ref:bonus:0" },
]
```

```rust
fn modify(args: (String, i32, Trace), bonus: &i32) -> (String, i32, Trace) {
    let (text, number, trace) = args;
    (text, number + *bonus, trace)
}
```

Tuple arity must equal the number of explicit arguments. A single argument uses
`(T,)`, and zero arguments use `()`. Context bindings do not become call
arguments. The generated typed adapter retains original argument expressions
outside added block scopes. Original argument and receiver expressions evaluate
once even if a later wrapper intentionally skips the operation. Owned values
need no Clone/Copy implementation; borrowed arguments remain borrow-checked.

All tuple modifiers at one site must agree on tuple and result types. Those
types must also agree with coexisting MODIFY_ARG and WRAP_OPERATION contracts.
Failures use `E_ARGUMENT_SIGNATURE`, `E_VALUE_SIGNATURE` or
`E_OPERATION_SIGNATURE`; Cargo checks actual callable types. `result` is
mandatory here and forbidden on other injection kinds.

## Local and parameter modification

MODIFY_VARIABLE selects `variable = { name, ordinal, at = "STORE" | "LOAD" }`,
with explicit require/ordinal for the access. STORE includes let initialization
and simple assignment RHS values. LOAD is initially restricted to primitive
scalar types and value expressions; references, place expressions and method
receivers cannot be rewritten as loads. `parameter = true` selects an incoming
parameter rather than a local declaration; parameter initialization is not a
STORE site. Compound/destructuring stores and unsupported declarations fail
closed. Modifiers use `$value` with equal input/output types. Capturing a binding
in its own initializer is invalid. Original access coordinates remain stable.

```toml
[[injection]]
id = "store"
kind = "MODIFY_VARIABLE"
target = "scoped_game::variables"
variable = { name = "health", ordinal = 0, at = "STORE" }
require = 2
ordinal = 1
helper = "store.rs"
bindings = [{ name = "value", ty = "i32", value = "$value" }]
```

```rust
fn modify(value: i32) -> i32 { value + 10 }
```

Here `variable.ordinal` identifies the declaration; the injection's `ordinal`
selects its second STORE among exactly two original stores. There is no
`selector` for this kind. To select an incoming parameter, add `parameter = true`
and use declaration ordinal zero. STORE modifies the new value before it is
assigned, preserving destruction of the prior value. LOAD modifies one original
value read without writing back. If a LOAD and STORE share an expression such
as `count = count`, the LOAD chain runs first regardless of cross-phase
priority. Within each chain, ordinary priority/mod/injection ordering applies.

LOAD supports primitive bool, char, integer and float types. Owned values and
place uses (including references, field bases and method receivers) are rejected
rather than risking moves or changes to borrowing. STORE supports owned values;
compound assignment, destructuring, fields and dereference stores are outside
this contract. Access counts include unsupported accesses to the selected
binding so selecting one reports an error instead of silently skipping it.
Identity/type failures use `E_LOCAL_NOT_FOUND` or `E_VARIABLE_TYPE`, count/ordinal
failures use `E_MATCH_COUNT`/`E_ORDINAL`, and unproven accesses use
`E_UNSUPPORTED_SITE` or `E_UNSUPPORTED_LOCAL`.

MODIFY_VARIABLE replaces only the value it selects (the read, or the value a
`let` or an assignment stores); the rest of the body is untouched. So, elsewhere
in the body, it allows a plain (not async) closure, a `?`, and a call beneath a
reference, as RETURN does outside its exit values. Inside the selected value all
three stay `E_UNSUPPORTED_CONTROL_FLOW`. An access inside a closure still counts
toward `require`, but selecting one is `E_UNSUPPORTED_LOCAL`: no hook runs inside
a closure. Locals declared inside a closure are not declarations of the function.

## Reusable field and method bridges

A mod may declare `[[bridge]]` entries, independently of injections. GET and
GET_MUT return shared/mutable references to a selected struct field; SET assigns
a value. INVOKE delegates to an exact existing inherent/trait impl method or
associated function. Each entry has an id, kind, target and (for fields) member.
Generated public inherent methods are named
`__horizon_mixins_<normalized_mod_id>_<normalized_bridge_id>`. Namespacing
collisions and existing method collisions fail planning. Types and methods must
be owned by the indexed workspace and unconditionally present. Generic types,
unsafe/async/const/foreign/generic methods and ambiguous ownership are rejected.
Bridges are emitted in the declaring module, preserving privacy and dependency
direction. Rust validates reference lifetimes, visibility of exposed types and
actual calls. No cross-crate private access or dependency is invented.

```toml
[mod]
id = "alpha"
version = "0.1.0"

[[bridge]]
id = "get-health"
kind = "GET"
target = "scoped_game::state::Entity"
member = "health"

[[bridge]]
id = "inc"
kind = "INVOKE"
target = "scoped_game::state::Entity::inc"
```

Consumers of the staged game crate can call:

```rust
let health: &i32 = entity.__horizon_mixins_alpha_get_health();
let increased_health: &i32 = entity.__horizon_mixins_alpha_inc(2);
```

IDs are lowercased and hyphens become underscores in generated names. Collisions
after normalization, including collisions with original methods, fail with
`E_BRIDGE_CONFLICT`. IDs are unique across a mod's injections and bridges.
Bridge-only bundles require no helper files. Different mods can generate
independent APIs for the same member. Helpers may call generated bridges;
defining functions or bindings in the reserved namespace remains forbidden.

Use a numeric member string such as `member = "0"` for tuple structs. GET_MUT
returns `&mut FieldType`; SET takes `FieldType` and assigns it. INVOKE omits
`member`, copies the original method signature, and delegates through its exact
inherent or trait impl. Associated functions returning `Self` act as factories.
Trait target IDs use the same `<Type as Trait>` syntax as injection targets.
Original method bodies may contain macros because the bridge only calls them.
The signature must be representable outside its original impl: unqualified
`Self::AssociatedType` projections require a concrete or fully qualified source
signature. Imported/aliased owner identities, enums and generic owners are not
resolved. Unsupported owners/signatures use `E_BRIDGE_TARGET`.

The generated method's public visibility exposes an ordinary API on the staged
type; it does not make the original field public. Calling original private
members from another crate still fails. Returned mutable references retain
normal exclusivity and lifetimes. No unsafe borrowing or runtime reflection is
generated, and no downstream dependency is added.

## Validation and composition

Every capture, access, boundary and bridge resolves against the pristine AST.
Repeated runs produce identical plans and generated source and preserve unchanged
file mtimes. OVERWRITE conflicts with every injection on its function; bridges
delegate to the final staged method and are not body insertions. Existing
cfg/macro/closure/async/generic injection safety gates still apply.

The 48-test integration suite compiles the earlier fixtures and this fixture,
including positive/negative scope, shadowing, privacy, exact-once evaluation,
owned value destruction and mutable-borrow examples. Source, bridge and schema-7
plan snapshots are checked independently of bundle discovery order. Plans are
review artifacts; `apply` and `check` rebuild them from original inputs.

For AFTER_ASSIGN, the captured environment is the original post-statement scope,
so the new result binding is available. RETURN capture validation includes all
interior cancellation exits rather than rejecting all captured cancellation
combinations. See [effects and state](effects-and-state.md).

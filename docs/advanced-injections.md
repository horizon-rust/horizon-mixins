# Advanced injection semantics

Version 0.2.0 implements the next five features after the MVP: cancellable
HEAD, RETURN value modification, MODIFY_ARG, MODIFY_CONSTANT, and WRAP_OPERATION.
The user explicitly requested these extensions; the original three-kind MVP
scope remains the baseline acceptance suite.

All sites resolve against pristine source. Ordering is priority descending, mod
ID ascending, injection ID ascending. OVERWRITE stays exclusive. No original
expression, receiver or argument is duplicated. Generated functions remain in
the target crate. Body-rewriting operations retain macro/cfg/async/closure/try/generic safety
checks. HEAD permits preserved body control flow as specified in
[boundary-head-and-modules.md](boundary-head-and-modules.md). Generated operation continuations are the compilation-tested exception
to the source-closure restriction. Current plans use schema 5; older plans must
be regenerated. Function definitions and bindings beginning `__horizon_mixins_`
are reserved; references to generated bridge APIs are allowed.

HEAD with `cancellable = true` returns `core::ops::ControlFlow<R>` (Continue(()),
or Break(value)), where R is the target return type. HEAD hooks run in ordinary
priority order; the first Break skips later HEAD hooks and the original body.
RETURN modifiers also run on that value.

RETURN modifiers apply to every explicit return and normal implicit function
result, including unit. One binding uses `value = "$value"`; its type and the
helper return type must equal the target result type. Other bindings may use
explicit target parameter context or validated lexical captures. Modifiers compose in execution order.
Original return expressions evaluate once. Non-unit functions without a value
tail require syntactically proven explicit exits. If an aliased unit result has
no such exits, write an explicit `()` tail; we do not guess alias resolution. Function-scope locals remain alive
while return modifiers run. Panics are not normal returns. `?`, async and a
`return` inside a macro remain unsupported rather than being silently missed; standard
macros elsewhere are preserved (see the macro section).

A RETURN modifier moves only the exit values (each `return` operand and the
function tail) into its helper's argument; the rest of the body is untouched.
So, elsewhere in the body, RETURN allows the first of these, and anywhere,
the second:

- a call beneath a reference, such as `decode(Some(&Tag::Compound(all)))` in a
  statement or `let x = &make();` whose temporary is extended: it keeps its
  temporary lifetime;
- a plain (not async) closure, such as `.filter(|s| ...)`: a `return` inside it
  leaves the closure, and RETURN does not rewrite it or treat it as an exit. What
  the closure holds is still checked, so `?`, async and unsafe inside it stay
  refused. Locals declared inside a closure do not count toward capture ordinals:
  `$local_ref:later:0` is the first `later` of the function itself.

A plain closure inside an exit value, such as the tail
`xs.iter().map(|x| x * 2).sum()`, moves into the helper's argument with the
value; its own `return` is still not an exit. A call beneath a reference inside
an exit value stays `E_UNSUPPORTED_CONTROL_FLOW`. A target that also
has an interior hook (BEFORE_CALL, AFTER_CALL, value and variable modifiers)
keeps the check over the whole body for that hook, since it rewrites there.

MODIFY_ARG selects a call with selector/require/ordinal and an explicit
zero-based `argument` index (excluding a method receiver). Its `$value` binding
and output have the same type. Other bindings may use target parameters or
explicit visible locals. Argument modifiers compose before tuple modifiers and
operation wrappers. Helpers are invoked via
a function-item expression, keeping original argument expressions outside any
new block scope and preserving receiver/argument evaluation order.

MODIFY_CONSTANT selects an exact Rust numeric, bool, char or string literal
expression (including signed numeric literals), with require/ordinal. Its
`$value` binding and output have the same type. Patterns and types are not
runtime value expressions; compile-time array lengths and const arguments
cannot be modified. Matching preserves spelling/suffixes and string content.

WRAP_OPERATION selects a call with selector/require/ordinal. Bind every original
argument exactly once using `$arg0`, `$arg1`, etc.; methods first bind
`$receiver`. The last binding is `$operation`, typed
`impl FnOnce(InputTypes...) -> OutputType`. Wrapper outputs and operation
signatures must agree at one site. No extra context bindings are allowed:
context can travel through operation arguments/receiver or module state.
Higher-priority wrappers are outermost. The original receiver/arguments evaluate
once, before wrappers run, and each wrapper can forward to the next operation
at most once or intentionally skip it. FnOnce is compiler-enforced.

Free-call wrapping requires a module-level callable path rather than a local
callable value. Method wrapping requires a target parameter receiver with an
explicit shared/mutable reference type matching the receiver binding; consuming
receivers and two-phase receiver/argument borrow patterns are unsupported.
Operation output must be an explicit owned type or unit. These restrictions
avoid guessing method resolution and lifetime behavior.

The combined pipeline is HEAD → original expression evaluation (constant and
argument modifiers at their original sites) → tuple modifiers → wrapper chain
→ RETURN modifiers.
BEFORE_CALL hooks at a wrapped call still execute before receiver/argument
evaluation. RETURN transformation and generated code are never revisited for
selector matching. The integration suite compiles positive/negative examples, checks composition
and order, and snapshots the generated source and schema-5 plan.

## Run the complete fixture

```sh
cargo run --locked -p horizon-mixins-cli -- check \
  --workspace fixtures/advanced --mods fixtures/advanced-mods \
  --out target/advanced-game --trust-native-mods
```

The two independent bundles declare 38 injections. Eight runtime tests check
all five features and their composition. The engine integration tests add
negative planning/compilation, temporary/local destruction ordering, nested
returns, immutable originals, deterministic plans/source, and unchanged mtimes.
Examples below are excerpts from those bundles; helpers are separate files
relative to each bundle manifest.

## Cancellable HEAD

```toml
[[injection]]
id = "cancel"
target = "advanced_game::calculate"
kind = "HEAD"
cancellable = true
helper = "cancel.rs"
bindings = [
  { name = "input", ty = "i32", value = "input" },
  { name = "log", ty = "&mut Vec<&'static str>", value = "&mut *log" },
]
```

```rust
fn cancel(input: i32, log: &mut Vec<&'static str>) -> std::ops::ControlFlow<i32> {
    log.push("cancel");
    if input == 0 { return std::ops::ControlFlow::Break(7); }
    std::ops::ControlFlow::Continue(())
}
```

`Continue(())` allows the next hook/body. `Break(7)` returns 7 through any RETURN
modifiers. No callback object, implicit capture or panic-based cancellation is
involved. `ControlFlow<()>` similarly supports unit-returning targets.

## RETURN / result modification

```toml
[[injection]]
id = "increase-result"
target = "advanced_game::exits"
kind = "RETURN"
helper = "result.rs"
bindings = [{ name = "value", ty = "i32", value = "$value" }]
```

```rust
fn modify(value: i32) -> i32 { value + 1 }
```

RETURN accepts no selector/count/ordinal: it modifies all normal exits. `$value`
is the original expression result, or the output of the preceding modifier.
Owned values move through the chain; no Clone/Copy is required. Borrowed return
values retain Rust's normal lifetime checking. Helper-local returns and generated
continuation returns are not target-function returns.

## MODIFY_ARG

```toml
[[injection]]
id = "increase-second-argument"
target = "advanced_game::arguments"
kind = "MODIFY_ARG"
selector = "sink"
require = 1
ordinal = 0
argument = 1
helper = "argument.rs"
bindings = [{ name = "value", ty = "i32", value = "$value" }]
```

```rust
fn modify(value: i32) -> i32 { value + 10 }
```

The receiver is excluded from `argument` numbering. The selected argument still
evaluates in its original position, once. A modifier's additional context must
be explicitly bound to target parameters or visible locals. Multiple modifiers of the same
argument must declare the same value type; their outputs feed the next helper.
Calls can be nested in larger expressions because these helpers do not wrap
original arguments in new blocks. Borrow conflicts still fail staged compilation.

## MODIFY_CONSTANT

```toml
[[injection]]
id = "increase-second-three"
target = "advanced_game::constants"
kind = "MODIFY_CONSTANT"
selector = "3"
require = 2
ordinal = 1
helper = "constant.rs"
bindings = [{ name = "value", ty = "i32", value = "$value" }]
```

```rust
fn modify(value: i32) -> i32 { value + 10 }
```

Selectors retain literal spelling and suffixes: `3`, `3i32`, and `0x3` are
separate syntax selectors. A negative number is one constant site; selecting
`2` does not accidentally match the magnitude of `-2`. For a string, use a TOML
string containing the Rust literal, e.g. `selector = '"two words"'`. Whitespace
inside strings is preserved. Runtime array elements can be modified; repeat
lengths and const arguments are rejected, and types/patterns/attributes are
excluded from matching.

## WRAP_OPERATION

```toml
[[injection]]
id = "around-call"
target = "advanced_game::combined"
kind = "WRAP_OPERATION"
selector = "base"
require = 1
ordinal = 0
helper = "around.rs"
bindings = [
  { name = "value", ty = "i32", value = "$arg0" },
  { name = "original", ty = "impl FnOnce(i32) -> i32", value = "$operation" },
]
```

```rust
fn around(value: i32, original: impl FnOnce(i32) -> i32) -> i32 {
    original(value * 2) + 5
}
```

Every wrapper sees the next continuation, rather than bypassing other mods.
Entry runs high priority to low priority; after-call code unwinds low to high.
Calling `original` twice is a Rust compile error. Skipping it is intentional
replacement behavior; the already evaluated original arguments still evaluate
once. This is an at-most-once API, not a runtime guarantee that a mod forwards.

A method's first binding is `$receiver`; its type must exactly match a target
reference parameter, including `&self`/`&mut self` resolved to a concrete type.
Then bind `$arg0` onward and `$operation` last. The continuation input types
include the receiver. Target-parameter shadowing, computed/consuming receivers,
borrowed results and two-phase receiver/argument borrow patterns are rejected.
See the compiled [method helper](../fixtures/advanced-mods/alpha/wrap-method.rs).

## Combined planning and compatibility

Constants are transformed at original literal coordinates. Argument modifiers
then apply at original call argument coordinates, followed by MODIFY_ARGS tuple
chains when present. See [scopes-and-bridges.md](scopes-and-bridges.md) for local
capture, slices and variable modification. BEFORE_CALL precedes receiver
and argument evaluation; wrappers run after those inputs evaluate. RETURN
processes a normal function exit, including cancellation. Priorities order
helpers within each phase/site; priorities do not change these phase semantics.

A wrapper signature mismatch reports `E_OPERATION_SIGNATURE`; incompatible
modifiers on one value report `E_VALUE_SIGNATURE`. Invalid argument indices use
`E_ARGUMENT`. These errors occur during planning. Actual Rust type, move and
borrow errors use the existing `E_BUILD` diagnostics with compiler output.
Opaque/inferred value types and unproven control-flow/sites are rejected.
OVERWRITE remains incompatible with every other kind on its function.

The [Sponge feature map](spongepowered-feature-gaps.md) records what remains
missing and distinguishes these Rust contracts from Java annotation behavior.

The 0.5.0 [effects and state extension](effects-and-state.md) adds expression-value
chains, after-call/after-assignment phases and interior ControlFlow cancellation.
RETURN also processes those cancellation exits; priorities order each phase.

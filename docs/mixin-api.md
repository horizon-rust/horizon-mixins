# Declarative MVP API

Mods are trusted Rust snippets described by TOML, rather than proc-macro
attributes. `--mods DIR` discovers direct `.toml` files and
`DIR/<bundle>/horizon-mixin.toml`. Discovery order has no semantic effect.
Unknown fields, duplicate IDs and missing helpers fail validation.

```toml
[mod]
id = "better-movement"
version = "0.1.0"
priority = 100 # default; each injection can override it

# Optional, exact 40-character commit; see the isolated Horizon prototype.
[compatibility]
horizon_revision = "028f60881678a9a0e3e198f693a82858fa475c76"

[[injection]]
id = "before-second-consume"
target = "fixture_game::calls::compute"
kind = "BEFORE_CALL"
selector = "consume"
ordinal = 1
require = 2
helper = "before.rs"
bindings = [{ name = "log", ty = "&mut Vec<&'static str>", value = "&mut *log" }]
# fingerprint = "<SHA-256 from the original plan>" # optional compatibility guard
```

`horizon_revision` is checked against Git HEAD or the `.horizon-revision` stamp
of a controlled source export. A stamp is provenance metadata, not cryptographic
attestation. The source files, Cargo.lock, toolchain file when present, and all
other staged inputs also have SHA-256 fingerprints in the plan.

Helper paths must be relative to their manifest directory, without traversal
or escapes through symlinks. A helper file contains exactly one private,
synchronous, safe function without attributes or explicit generic parameters.
WRAP_OPERATION's continuation parameter uses `impl FnOnce` as specified below. It becomes a local
function in a separate block inside the target. Independent helpers can use
the same Rust function name. Module types, imports and private fields are in
scope; outer function locals and `Self` are not implicit context.

```rust
fn before(log: &mut Vec<&'static str>) {
    log.push("before");
}
```

Every helper argument needs a binding in declaration order. `name` and `ty`
must exactly match its parsed parameter name/type. `value` is a target parameter,
its borrow, or explicit dereference/reborrow (`self`, `&self`, `&mut *entity`).
Calls and fields are not binding expressions. Local declarations use the explicit
capture selectors below. Use concrete types in helper signatures. A parameter
binding must still refer to that parameter at each selected site; shadowing there
is rejected.
Rust compilation validates the actual types, mutability, lifetimes and moves.
Value/operation bindings beginning with `$` are described in
[advanced-injections.md](advanced-injections.md). Local selectors
`$local:NAME:ORDINAL`, `$local_ref:NAME:ORDINAL` and `$local_mut:NAME:ORDINAL`
are documented in [scopes-and-bridges.md](scopes-and-bridges.md).

HEAD inserts helper calls before the original first statement. BEFORE_CALL
inserts before receiver/callee and argument evaluation, preserving one original
call as the final expression. OVERWRITE replaces only the body with a helper
call as its tail, retaining the original attributes/signature. Ordinary insertion helpers
return `()`; cancellable HEAD and value/wrapper helpers follow their typed
contracts. Overwrite return compatibility is checked by Cargo. HEAD and
OVERWRITE do not accept selector/count/ordinal fields.

Function IDs include Cargo target name (hyphens become underscores), module,
impl identity, and function:

| Source | Target ID |
|---|---|
| Free function | `fixture_game::entity::advance` |
| Inherent method | `fixture_game::entity::Entity::tick` |
| Trait impl method | `fixture_game::entity::<Entity as Marker>::marked` |

Selectors are syntax, not semantic resolution: `consume`, `crate::consume`,
`entity.apply_friction`, or `make_receiver(evaluations).take`. Whitespace outside literals is not accepted. Spaces inside literal strings are
preserved and significant. `require` counts all exact matches in the original body, and
`ordinal` selects one in AST preorder. With a slice, this count and ordinal apply
inside the original region; each boundary has its own global count and ordinal.
A selector is never matched against an
injected helper. Counts and ordinals are mandatory even when only one match
is expected. Bare method names do not match qualified receiver expressions.

BEFORE_CALL supports complete expression statements, plain identifier `let`
initializers and block/function tails. Other placements fail with
`E_UNSUPPORTED_SITE`. Functions containing opaque macros, unresolved cfg,
closures, async, await, `?`/try, unsafe/const blocks, nested items or calls under
references are outside the proven MVP subset. RETURN is narrower: it allows
plain closures, and refuses calls under references only inside the values it
moves (each `return` operand and the function tail); see
[advanced injections](advanced-injections.md). MODIFY_VARIABLE refuses closures, `?`
and calls under references only inside the value it modifies; see
[scopes and bridges](scopes-and-bridges.md). Generic, const, unsafe and
foreign-ABI target functions are also rejected. Feature-gated module trees
and Cargo targets are not guessed or expanded.

The advanced API implements cancellable HEAD, RETURN, MODIFY_ARG,
MODIFY_CONSTANT and WRAP_OPERATION; see [contracts and runnable
examples](advanced-injections.md). Version 0.3.0 adds local capture, slices,
MODIFY_ARGS, MODIFY_VARIABLE and reusable accessor/invoker bridges; see
[scoped contracts and examples](scopes-and-bridges.md). A bundle may contain
injections, bridges, support modules and/or state; IDs are unique within a bundle.
Plans use schema 5, including lexical identities, original expression
coordinates, resolved regions and versioned bridge metadata. Regenerate older
plans after upgrading.

Future attribute syntax, additional dependencies, semantic selectors, REDIRECT
and broader structural transformations require separate
specifications and tests. The [SpongePowered feature map](spongepowered-feature-gaps.md)
documents the remaining gaps.

HEAD has a compilation-tested exception for preserved closure/`?` bodies and
lifetime-bearing impls. HEAD-only hooks, including cancellable HEAD, additionally
permit opaque expression and statement macros such as `format!`, `matches!` and
`vec!` in the preserved body. Version 0.4.0 also adds `[[module]]` for shared same-crate
implementation code. See [boundary contracts](boundary-head-and-modules.md).
Support modules default to `pub(crate)`; `visibility = "pub"` exposes a module
through ordinary Rust visibility to existing dependent crates. Payload items
must also be public, and no Cargo dependencies are added.

Version 0.5.0 adds `MODIFY_EXPRESSION_VALUE`, `AFTER_CALL`, `AFTER_ASSIGN`,
`cancellable` interior hooks and `[[state]]` declarations. See
[effects and state](effects-and-state.md) for explicit result contracts,
post-assignment captures, constructor counts, initialization and serialization.
CLI success reports now include `states` as well as injections/bridges/modules.

Body-rewriting hooks can coexist with validated `format!`, `matches!` and `vec!`
inputs. Macro invocations stay unchanged; hidden control flow, ambiguous macro
identity and selectors matching macro internals are rejected. See
[standard macro contracts](standard-macros.md).

Version 0.6.0 adds `hook`/`hook_version` (exactly one of `target` or `hook` per injection),
`[[crate]]` and `[[post_build]]`, and stages a bundle's `data/` folder. A bundle may now consist of
crates only. CLI success reports include `crates`, `data` and `post_build` counts, and plans use
schema 6. See [named hooks, bundle crates, data and post-build steps](crates-data-and-named-hooks.md).

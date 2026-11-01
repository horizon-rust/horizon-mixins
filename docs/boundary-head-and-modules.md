# Lifetime methods, boundary HEAD hooks and support modules (0.4.0)

Veinminer is the first full-game consumer of this extension. Plans use schema 7;
regenerate older plans. It declares three cancellable HEAD injections and one
support module. All changes to the exported Horizon Rust source are performed
by the mixin planner/transformer and atomic staging CLI.

## Lifetime-bearing methods

Inherent impls may declare lifetime parameters, including `impl<'a> Game<'a>`.
Their self-type IDs normalize non-static lifetimes to `'_`, so a renamed
lifetime does not change a target ID. A method in an external `commands` module
is identified by its declaring module:

```toml
[[injection]]
id = "command"
kind = "HEAD"
cancellable = true
target = "horizon_client::game::commands::Game<'_>::run_command"
helper = "command.rs"
bindings = [
  { name = "game", ty = "&mut crate::game::Game<'_>", value = "&mut *self" },
  { name = "command", ty = "&str", value = "command" },
]
```

Type/const generic impls, impl where clauses, explicitly generic methods and
async/unsafe/const/foreign targets remain unsupported. Helpers still use
concrete types and do not capture the target's named lifetime parameters.
Compiler checking is required for actual lifetime and borrow compatibility.

## Preserved body control flow

HEAD can prepend a call before a body containing closures, `?`, reference
expressions or nested ordinary items. It does not inspect or rewrite their
sites or exits. If a cancellable hook continues, the original body executes
with its original returns, error propagation and destruction behavior. If it
breaks, normal Rust return semantics drop the target's owned arguments once.

HEAD-only hooks, including cancellable HEAD, also support opaque expression and
statement macro invocations, such as `format!`, `matches!`, `vec!`, `assert!`
and macro-generated early returns. Their tokens remain untouched; no expansion or selection inside them
is performed. Signature, type, pattern and item macros (including local
`macro_rules!` definitions), unresolved cfg and attribute macros still fail
closed. Attributes on macro invocations are checked as usual. Cancelling HEAD
returns before any original macro or local initializer executes. Async functions
remain rejected. RETURN and site injections continue to use their stricter control-flow validation, including
when combined with HEAD. HEAD-only planning does not assign expression/local
coordinates inside the untouched body; the transform skips expression traversal
when there are no planned sites.

The compilation fixture tests two mods in priority order, closure errors,
owned argument destruction, cancellation, stable lifetime IDs and deterministic
source/mtimes. It also tests rejection of RETURN over `?`, type generics,
unsupported macro contexts, ambiguous module destinations and unsupported
payloads. Macro HEAD tests compile two ordered callbacks, expression/statement
macros, macro-generated early returns and tail values, and local/argument drops.
Both list/repeat `vec!` forms and `format!` arguments execute exactly once;
cancellable hooks exercise both Continue and Break before those expressions.
Removing only the generated HEAD calls reproduces the original body AST;
repeated staging preserves bytes and mtimes.

This is preservation, not macro expansion: selectors cannot target calls or
locals inside macro tokens. Body-rewriting hooks now accept `format!`, `matches!`
and `vec!` after validating their identity and inputs. Hidden exits or selector
matches are still rejected; see [standard macro contracts](standard-macros.md).

## Support modules

A bundle can contribute a self-contained Rust module to the top level of an
existing indexed source file:

```toml
[[module]]
id = "runtime"
file = "crates/horizon-client/src/game.rs"
name = "veinminer"
source = "runtime.rs"
```

`source` is relative to the bundle and contains the module's items, without an
outer module wrapper. The transformer emits `pub(crate) mod veinminer { ... }`
after all edits to original functions and bridges. Its types/functions are
available to helpers through ordinary Rust paths and same-crate privacy.
An optional `visibility` accepts exactly `"pub(crate)"` (the default) or `"pub"`.
For a support API used by a hook in another crate, declare it in a library file
without top-level glob imports. This example uses the two-crate test workspace
in `fixtures/game`, where `fixture-driver` already depends on `fixture-game`:

```toml
[[module]]
id = "runtime"
file = "engine/src/lib.rs"
name = "support"
source = "runtime.rs"
visibility = "pub"
```

With `runtime.rs` containing `pub fn touch(value: &mut i32) { *value += 2; }`,
this emits `pub mod support { ... }`. A hook in `fixture-driver` can call
`fixture_game::support::touch(value)`. The
[`public_support_module_is_callable_from_a_hook_in_a_dependent_crate` test](../crates/horizon-mixins-cli/tests/cases/boundary.rs)
compiles and exercises this arrangement. Public module visibility does not make
its items or parent modules public. Rust still checks item/type visibility and
dependency direction; no reverse dependency or cycle is introduced automatically.

The planner conservatively rejects any top-level glob import in a destination
file with `E_MODULE_CONFLICT`, because it cannot prove which names the glob
introduces. This includes cfg-gated imports: Horizon's
`crates/horizon-mods/src/lib.rs` contains `pub use runtime::*;`, so that file
cannot currently receive a support module, even with `visibility = "pub"`.
Choose an indexed destination without glob imports whose parent modules are
public when cross-crate access is needed.

It adds no Cargo dependencies or fields to existing structs. IDs are unique
across injections, bridges and modules in a bundle.

The destination must have exactly one unconditional source identity. Existing
type/module/import names, competing module names and unresolved glob imports
are rejected. Payloads are parsed during planning. External `mod`, `#[path]`,
direct include macros, file attributes and shebangs are unsupported; assemble
multi-file mod implementations into a self-contained payload before planning.
Ordinary Rust macros and cfg inside new implementation code are compiled by
Cargo; injected helpers are never matched against that code.

Plans contain module identity/version, destination fingerprint and source, plus
`visibility = "pub"` for public modules. Omitted plan visibility means
`pub(crate)`; regenerated plans keep this default and generated output unchanged.
Original fingerprints are checked before function rewrites; module insertion
happens afterward so original node paths cannot shift. Changed support code
changes the plan and staged file; repeated identical runs preserve bytes and
mtimes. `E_MODULE` and `E_MODULE_CONFLICT` describe payload/destination errors.
Saved JSON remains a review artifact, not an executable CLI input.

## Veinminer integration

The separate `horizon-veinminer` repository commits `mixins/veinminer/` with its
manifest, three typed hooks and runtime payload. Its development tool exports
committed Horizon files, then invokes `horizon-mixins plan` and `apply`; it has
no custom Rust-source replacement logic. The runtime keeps UUID-keyed toggles
and cooldowns in per-world state on the hosting thread, avoiding any change to
`Game` fields or constructors. Extras call the unchanged native mining method.

Native validation type-checks the real pinned game binary and runs the 13 mining
engine tests embedded in it. A separate syn audit removes only the three planned
HEAD statements and module, then compares every affected file's AST with the
pristine export. Gameplay sessions with prepared assets remain a manual check;
no game assets or upstream source edits are needed for these checks.

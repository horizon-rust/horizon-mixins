# AST rewriting and conflict semantics

All targets and sites resolve on pristine source. A function has a stable
module/impl identity, source item-index path, and signature/body fingerprint.
A call has an original preorder index and syntax fingerprint. Rewriting never
matches helpers emitted by another injection.

Compatible same-site inserts execute in priority descending, mod ID ascending,
injection ID ascending order. Priorities never select an overwrite winner.
Two overwrites, or OVERWRITE plus any other hook on the same physical function,
fail with `E_OVERWRITE_CONFLICT` during planning, before stage publication.

HEAD prepends separate blocks, each defining and invoking a local helper.
OVERWRITE replaces the body with a local helper and a tail call, retaining the
surrounding original function signature and attributes.

BEFORE_CALL replaces a selected call with an expression block:

```rust
let result = {
    { fn before() {} before() };
    original(argument())
};
```

The hook runs before receiver/callee and arguments, and the original call is
emitted exactly once. Its result remains the tail expression. For a discarded
expression statement, the original call instead retains a semicolon inside the
generated block; this preserves destruction of a returned value before its
receiver temporary. A separate compiled Drop-tracing test covers that order. The compiled
fixture tests both a function call and a side-effecting method receiver, checks
argument/receiver counts and returned values, and uses two hooks at one call
site. A helper deliberately contains another matching call to prove it does
not change original `require` or `ordinal` resolution.

An extra block can shorten temporary lifetimes or change drop order inside a
larger expression. Consequently the selected call must be the complete
expression of a statement, a plain identifier `let` initializer, or a block or
function tail (parentheses are allowed). Binary operands, call arguments,
conditions and match-arm expressions are rejected as `E_UNSUPPORTED_SITE`.
Reference-wrapped calls, closures/async/await/try/`?`, nested items, unsafe/const
contexts, cfg and opaque macros fail closed. Parameter shadowing is rejected
for BEFORE_CALL so bindings cannot silently refer to a different local.
Compilation remains necessary to validate actual borrowing and return types.

Representative diagnostics:

| Code | Meaning |
|---|---|
| `E_TARGET_NOT_FOUND` | Exact source function is absent |
| `E_TARGET_AMBIGUOUS` | Multiple definitions or shared source identities |
| `E_UNSUPPORTED_MACRO_SITE` | A selector, variable or state constructor also occurs inside preserved standard macro inputs |
| `E_MATCH_COUNT`, `E_ORDINAL` | Selector compatibility mismatch |
| `E_OVERWRITE_CONFLICT` | Overwrite is not exclusive |
| `E_UNSUPPORTED_MACRO`, `E_UNSUPPORTED_CFG` | Syntax cannot be resolved safely |
| `E_UNSUPPORTED_SIGNATURE`, `E_UNSUPPORTED_CONTROL_FLOW`, `E_UNSUPPORTED_SITE` | Unsupported source context |
| `E_MANIFEST`, `E_DUPLICATE_ID`, `E_INVALID_HELPER`, `E_INVALID_BINDING` | Invalid mod declaration |
| `E_INVALID_PATH`, `E_STAGE_OWNERSHIP` | Unsafe input/output location |
| `E_UNPINNED`, `E_REVISION`, `E_FINGERPRINT`, `E_STALE_SOURCE` | Compatibility/source mismatch |
| `E_BUILD` | Staged Cargo failed; compiler output included |

Diagnostics carry code/message and applicable file, mod, injection, target and
selector. The CLI exits nonzero and prints JSON on stderr. Source and plan
snapshots live in `crates/horizon-mixins-cli/tests/snapshots`; update deliberately
with `UPDATE_SNAPSHOTS=1 cargo test --workspace --locked`.

Version 0.2.0 adds [advanced injection contracts](advanced-injections.md).
Value modifiers use a scoped function-item callee with original values as
ordinary call arguments, avoiding an added block around original expressions.
Operation wrappers use typed, generated FnOnce continuations. Return modifiers
run on explicit exits, implicit tails and cancelled HEAD results; generated
helper/continuation returns are not revisited. Current schema-7 plans store
value/output contracts, argument indices and original constant coordinates,
plus lexical/access identities, captures, resolved regions and bridge metadata.
Regenerate older plans after upgrading.

`E_ARGUMENT` diagnoses invalid MODIFY_ARG indices; `E_VALUE_SIGNATURE` and
`E_OPERATION_SIGNATURE` diagnose incompatible composition. Function definitions
and bindings beginning `__horizon_mixins_` are reserved (`E_RESERVED_NAME`);
references to generated bridge APIs are allowed. The original cfg/macro/
source safety checks and exclusive-overwrite rule remain in force.

Version 0.3.0 implements [scoped injections and bridges](scopes-and-bridges.md).
Scopes resolve declaration identities before rewriting, so a capture cannot
silently bind a shadowing declaration. Variable LOAD and STORE phases use
original expression coordinates; LOAD runs before STORE if both select one
expression. Individual argument modifiers precede tuple modifiers and operation
wrappers. Slices filter original sites between validated call boundaries before
the injection's own count/ordinal checks. Namespaced public bridge methods are
appended to the owning module after function rewriting. Bridges delegate to the
staged method, including any planned body changes, and require no Cargo edge.

`E_LOCAL_NOT_FOUND`, `E_LOCAL_NOT_VISIBLE`, `E_CAPTURE_TYPE` and
`E_UNSUPPORTED_LOCAL` diagnose lexical/access limitations. `E_VARIABLE_TYPE`
and `E_ARGUMENT_SIGNATURE` diagnose value/tuple contracts. `E_SLICE_*` errors
describe invalid boundaries, scopes and order; `E_BRIDGE_TARGET` and
`E_BRIDGE_CONFLICT` describe unsupported owners/signatures and name collisions.
Rust type, lifetime, borrowing and move errors still come from staged Cargo.

Version 0.5.0 adds [effects and state](effects-and-state.md). Typed AFTER_CALL
adapters observe the original result before consumption; AFTER_ASSIGN statements
run after its store and temporary destruction. Expression-value modifiers run
after call adapters and before STORE modifiers. Interior cancellation rewrites
to a real target return, including RETURN processing and ordinary Rust drops.
Original struct literals receive deterministically ordered state initializers;
struct fields and accessors are added after body rewrites. `E_AFTER_RESULT`
diagnoses invalid result borrow contracts; `E_STATE` diagnoses unsupported owner,
constructor, initialization and pattern cases; `E_STATE_CONFLICT` rejects
namespaced collisions and OVERWRITE removing a constructor. Cargo checks types,
moves, derives, serialization and any constructions hidden behind aliases/macros.

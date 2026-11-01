# Architecture

The implemented workspace has three crates:

- `horizon-mixins-core`: strict manifests, source snapshots, module/function
  indexing, original-site IR, conflict detection and structured diagnostics.
- `horizon-mixins-transform`: pure `syn` AST rewrite and `prettyplease` output.
- `horizon-mixins-cli`: `plan`, `apply`, `check`, atomic staging and Cargo locks.

The planner snapshots inputs, asks `cargo_metadata` for workspace libraries and
binaries with `--no-deps --locked --offline`, and walks their original module
trees. It handles inline modules, external `mod.rs`/`name.rs`, `#[path]`, free
functions, inherent impls and trait impls. It rejects external path dependencies,
recursive/escaping module files, ambiguous identities and source files compiled
under more than one module/target identity. Tests/examples/benches are copied
and compiled by Cargo but are not injection targets in the MVP.

Every injection, bridge and state constructor resolves before any transformation. The schema-7 JSON plan
records transformer/rustc version, every input file's SHA-256, mod identity and
version, target ID, original file and item-index path, signature/body fingerprint,
call preorder number/fingerprint, selector/count/ordinal, priority, helper source,
explicit bindings, cancellation/argument options, constant coordinates, and
typed value/output/tuple/continuation contracts. It also records original
expression coordinates, lexical binding/access identities, capture expressions,
resolved slice boundaries, and versioned bridge declarations with source
fingerprints and generated methods. It contains no absolute checkout paths or discovery paths,
so bundle enumeration does not affect serialized bytes.

The transformer verifies input/toolchain fingerprints, groups patches by source
function, numbers original calls, literals and expressions in preorder sequences, and rewrites their
children before replacing each selected node. It never traverses generated
helper code. It applies constants/arguments/operation wrappers on original coordinates, then
rewrites function exits, then inserts HEAD statements. Generated helper functions
and continuations are excluded from exit rewriting.
This preserves original coordinates even when hooks contain matching calls.
The scope analysis and rewrite share the same expression traversal. LOAD
modification precedes STORE modification when they select the same expression.
Per-argument chains precede tuple chains, which precede operation wrappers.
Bridge source fingerprints are checked before rewriting; generated impls are
appended in their owning module after all function edits, keeping item paths
stable. Only files with selected functions, bridges, modules or state are pretty-printed;
other files retain bytes.
Non-doc comments in transformed files are not preserved by `syn`/`prettyplease`.

The privacy bridge is a local function inside the patched body. Explicit
parameter bindings or explicit lexical captures supply context, with Rust enforcing privacy, moves,
borrows and return types during `check`. No mod crate becomes a dependency of
the game, so this design introduces no Cargo cycle. The phase-0 two-crate fixture
proves private field access/reborrowing and checks that cross-crate private
access and circular dependencies actually fail. Native helpers use the target
crate's existing dependency/import environment; adding dependencies is future work.
Reusable bridges add namespaced public inherent methods beside an owned struct
or original impl. They access fields or delegate to the original method there,
so ordinary Rust privacy and borrowing still apply to callers. No mod crate or
dependency edge is generated.

The CLI rebuilds a validated plan for each invocation, transforms in memory,
then publishes a complete separate workspace. Saved JSON plans are reviewable
artifacts and are not executable CLI inputs. The transform library validates
IR structure and original fingerprints before traversing it; callers should
obtain semantic plans through `build_plan`.

Advanced semantics and protocol details are in
[advanced-injections.md](advanced-injections.md) and
[scopes-and-bridges.md](scopes-and-bridges.md); upstream feature gaps are in
[spongepowered-feature-gaps.md](spongepowered-feature-gaps.md).

See [staging](build-and-cache.md), [supported bindings](mixin-api.md), and
[prototype results](implementation.md). Compiler-aided resolution, launcher
protocols and fingerprinted executable publication remain future work.

Boundary-only HEAD validation and support-module planning are described in
[boundary-head-and-modules.md](boundary-head-and-modules.md). Modules are
fingerprinted before rewrites and appended afterward; original item paths stay
stable. Lifetime-only impls have canonical self-type identities.

Schema 5 includes explicit AFTER_CALL result types and state owner/constructor
fingerprints. Expression modifiers use original expression coordinates. AFTER_ASSIGN
captures resolve in the original post-statement environment; generated statements
are appended after that statement and never traversed. Interior cancellation
returns from the target; exit rewriting also processes those generated exits.
State literal fields are inserted after original child traversal, with owner
fields/getters/initializer functions appended after all body changes. This keeps
all original function/expression indices stable. Struct updates move existing
state through their rest expression. See [effects and state](effects-and-state.md).

`horizon-mixins-core::native` resolves named hooks from `native-hooks.json` before
indexing, and validates bundle crates, same-bundle dependency edges, data and post-build steps
against the workspace's members and `Cargo.lock`. It catalogs all declared crate roots before
resolving normal/build/dev path dependencies, rejects cycles and unsafe paths, and plans edges by
table, alias key and target crate ID. After the transform, `native::stage` verifies the original
bundle hashes and edge correspondence, rewrites sibling paths relative to staged crate IDs, adds
the bundle files, and edits workspace/dependent manifests and `Cargo.lock` in place.

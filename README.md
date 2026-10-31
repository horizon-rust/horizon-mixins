# horizon-mixins

A working stable-Rust CLI for deterministic, build-time source mixins for
[Horizon](https://github.com/ludovic111/horizon). The MVP implements declarative
TOML mods, HEAD and interior cancellation, BEFORE_CALL, AFTER_CALL, AFTER_ASSIGN,
MODIFY_EXPRESSION_VALUE, RETURN, MODIFY_ARG,
MODIFY_ARGS, MODIFY_CONSTANT, MODIFY_VARIABLE, WRAP_OPERATION, OVERWRITE,
lexical local capture, slices, accessor/invoker bridges, per-instance state, JSON planning and atomic
Linux staging. Native mods run with your user privileges.

```text
Pristine Cargo workspace + independent mod bundles
    → index source → validate combined plan → transform AST
    → separate staged workspace → cargo test --locked --offline
```

The three workspace crates are `horizon-mixins-core` (manifests, indexing,
planning, diagnostics), `horizon-mixins-transform` (AST rewriting), and
`horizon-mixins-cli` (the `horizon-mixins` binary). Rust 1.95.0 and Cargo.lock
are pinned. Cargo metadata and builds use offline, locked dependency resolution.
Run `cargo fetch --locked` once to populate the tool's dependencies.

Build and run the complete two-mod fixture:

```sh
cargo build --locked -p horizon-mixins-cli
./target/debug/horizon-mixins plan \
  --workspace fixtures/game --mods fixtures/mods --out target/plan.json
./target/debug/horizon-mixins apply \
  --workspace fixtures/game --mods fixtures/mods --out target/modded-game
./target/debug/horizon-mixins check \
  --workspace fixtures/game --mods fixtures/mods --out target/modded-game \
  --trust-native-mods
```

`plan` writes the full validated plan to `--out`. `apply` and `check` rebuild
that plan from pristine source, then stage all input files. They never patch
the original checkout. `check` executes the staged workspace's tests; it
requires explicit native-code trust. Successful commands print JSON to stdout;
failures print a structured diagnostic to stderr and exit nonzero. JSON plans
are review artifacts; the CLI does not execute saved plans.

`check --package NAME` limits Cargo to a package; `--target-dir PATH` selects
a reusable, locked target directory. The default target directory is a stable
sibling of the stage. Outputs, mods and originals must be separate. Existing
output directories need the generated stage marker. Unchanged files keep their
bytes and mtimes, including across publication of changed stages.

A bundle contains `horizon-mixin.toml` and helper `.rs` files. For example:

```toml
[mod]
id = "movement"
version = "0.1.0"
priority = 100

[[injection]]
id = "head"
target = "fixture_game::entity::advance"
kind = "HEAD"
helper = "head.rs"
bindings = [{ name = "entity", ty = "&mut Entity", value = "&mut *entity" }]
```

```rust
// head.rs: compiled as a local function inside the target function.
fn head(entity: &mut Entity) {
    entity.velocity += 3; // Private fields in the target module are accessible.
}
```

Helpers use explicit typed parameter bindings. They are compiled in the game
crate and introduce no game→mod→game Cargo cycle. All selectors resolve on the
original AST; same-site hooks run by priority descending, mod ID ascending,
then injection ID ascending. An OVERWRITE conflicts with every other hook on
its function.

BEFORE_CALL requires an exact syntax selector, explicit total `require`, and
zero-based `ordinal`. It supports a complete expression statement, plain `let`
initializer, or block/function tail. Calls nested in larger expressions are
rejected because wrapping them can change temporary drop timing. For BEFORE_CALL, macro
sites, unresolved cfg, async/closures/`?`, generic/unsafe/const targets, and
ambiguous source identities are rejected (RETURN allows plain closures, and references to
calls outside the values it returns, and MODIFY_VARIABLE closures, `?` and references
to calls outside the value it modifies). Rust checks privacy, types and borrowing in
`check`; successful planning alone does not guarantee compilation.

The five advanced features have a separate compilation fixture:

```sh
cargo run --locked -p horizon-mixins-cli -- check \
  --workspace fixtures/advanced --mods fixtures/advanced-mods \
  --out target/advanced-game --trust-native-mods
```

See [advanced injection contracts and examples](docs/advanced-injections.md)
and the [SpongePowered feature map / remaining gaps](docs/spongepowered-feature-gaps.md).

Version 0.3.0 adds local capture, slices, reusable field/method bridges, joint
argument modification and local/parameter STORE and scalar LOAD modification:

```sh
cargo run --locked -p horizon-mixins-cli -- check \
  --workspace fixtures/scoped --mods fixtures/scoped-mods \
  --out target/scoped-game --trust-native-mods
```

The [scoped injection and bridge guide](docs/scopes-and-bridges.md) documents
the exact contracts and limits. Plans use schema 5; regenerate older plans.

Validate the implementation:

```sh
python3 tools/feasibility.py
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
python3 tools/benchmark.py # fresh target/benchmark; use --out for another run
```

The 89-test integration suite includes generated source/plan snapshots, positive and
negative compilation, deterministic ordering, immutable originals, stale-source
rejection, concurrent checks, and unchanged mtimes. Linux CI runs the same checks.

An isolated test against the complete `horizon-core` crate from commit
`028f60881678a9a0e3e198f693a82858fa475c76` passed, including two independent
HEAD hooks in `jmath::atan`. Reproduce it using your local clone:

```sh
python3 tools/horizon_prototype.py --horizon /home/levzzz/Documents/horizon
```

It exports committed crate sources into a fresh `target/horizon-prototype`,
uses an isolated workspace manifest/lockfile, runs upstream tests and generated
probes, and records timings. It does not build the full game or launcher.
No upstream checkout is edited; no Minecraft assets or prepared resources are used.

Further documentation: [API](docs/mixin-api.md),
[transformations](docs/transformations.md), [architecture](docs/architecture.md),
[staging and cache](docs/build-and-cache.md), [security](docs/security.md),
[roadmap](docs/roadmap.md), [feasibility and measurements](docs/implementation.md).
Additional injection kinds, broader structural mixins, launcher integration, semantic
resolution, and a complete executable cache remain future work. License: [MIT](LICENSE).

Version 0.4.0 supports lifetime-bearing impl methods, HEAD over preserved
closure/`?` bodies, and declarative same-crate support modules.
HEAD-only hooks, including cancellable HEAD, also preserve body expression/statement
macros such as `format!`, `matches!` and `vec!`; body-rewriting hooks also accept
these three after [input and identity validation](docs/standard-macros.md). See
[boundary HEAD and support modules](docs/boundary-head-and-modules.md).
The separate [Veinminer port](https://github.com/levzzz5154/horizon-veinminer)
uses three TOML HEAD hooks and one module on the pinned complete Horizon game.

Version 0.5.0 adds expression-result modification, after-call/after-assignment
hooks, interior cancellation and namespaced per-instance state with explicit
constructors and serde defaults. See [effects and state](docs/effects-and-state.md)
for exact contracts, composition order, save-data behavior and supported limits.

```sh
cargo fetch --locked --manifest-path fixtures/effects/Cargo.toml
cargo run --locked -p horizon-mixins-cli -- check \
  --workspace fixtures/effects --mods fixtures/effects-mods \
  --out target/effects-game --trust-native-mods
```

Version 0.6.0 adds what larger native mods need: a named `hook` + `hook_version` from the
workspace's `native-hooks.json` instead of a raw target, the bundle's own crates (`[[crate]]`,
staged as workspace members with registry dependencies taken only from the existing `Cargo.lock`),
`data/` files staged to `native-data/<mod-id>/`, and `[[post_build]]` steps written to
`native-post-build.json` for the launcher (the tool never runs them). Plans use schema 6. See
[named hooks, bundle crates, data and post-build steps](docs/crates-data-and-named-hooks.md).

```sh
cargo run --locked -p horizon-mixins-cli -- check \
  --workspace fixtures/native --mods fixtures/native-mods \
  --out target/native-game --trust-native-mods
```

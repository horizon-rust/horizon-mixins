# Named hooks, bundle crates, sibling dependencies, data and post-build steps

Version 0.6.0 lets a native bundle grow past a few helper files:

- a **named hook** (`hook` + `hook_version`) instead of a raw function path, resolved from the
  workspace's `native-hooks.json`;
- its **own crates** (`[[crate]]`), staged as workspace members and built separately from the game
  crate, with registry dependencies taken from the workspace's existing `Cargo.lock`;
- **data files** (`data/`), staged to `native-data/<mod-id>/` for `include_bytes!`;
- **post-build steps** (`[[post_build]]`), recorded in `native-post-build.json` for the launcher.

`horizon-mixins` never runs a post-build step. Everything below follows the existing staging rules:
the stage is published atomically, the output is deterministic, unchanged files keep their bytes and
mtimes, symlinks and path escapes fail, and every failure is a structured diagnostic. Plans use
schema 7, which records resolved same-bundle dependency edges.

The compiled example is `fixtures/native` with `fixtures/native-mods/boost`:

```sh
cargo fetch --locked --manifest-path fixtures/native/Cargo.toml
cargo run --locked -p horizon-mixins-cli -- check \
  --workspace fixtures/native --mods fixtures/native-mods \
  --out target/native-game --trust-native-mods
```

## Named hooks

```toml
[[injection]]
id = "tick"
hook = "tick"        # a name from <workspace>/native-hooks.json
hook_version = 1     # required with hook
kind = "HEAD"
helper = "tick.rs"
bindings = [{ name = "game", ty = "&mut Game", value = "self" }]
```

The catalog is the one Horizon keeps at its repository root
([horizon-rust/horizon#3871](https://github.com/horizon-rust/horizon/pull/3871)):

```json
{"hooks": [{"name": "tick", "version": 1, "target": "horizon_client::game::native_hooks::Game<'_>::native_tick"}]}
```

Before planning, each `(hook, hook_version)` is replaced by its `target`, so the rest of planning is
unchanged. The plan keeps the resolved `target` and also records `hook` and `hook_version`. A hook
version is a contract (signature, timing, behaviour). Horizon keeps an old version, or adds a new one
when the contract changes incompatibly. So a bundle fails at plan time with a clear message instead
of compiling against a changed hook.

The catalog is read only when a manifest names a hook. Other fields in its entries are ignored.

| Code | Cause |
|---|---|
| `E_MANIFEST` | both `target` and `hook`; `hook` without `hook_version` (or the reverse); neither |
| `E_HOOK_CATALOG` | a hook is used but the workspace has no `native-hooks.json`, or it is invalid JSON, lists an entry without a name/target or with version 0, or lists a name/version twice |
| `E_HOOK_UNKNOWN` | the name is not in the catalog; the message lists the known names |
| `E_HOOK_VERSION` | the name is known but not at that version; the message lists the versions it has |

## Bundle crates

```toml
[[crate]]
id = "engine"                 # shares the id space with injections, modules, bridges, states
path = "engine"               # bundle-relative folder holding Cargo.toml
dependent = "native-game"     # workspace package that depends on it (default "horizon-client")
```

The folder's files are copied to `native-crates/<mod-id>/<crate-id>/`. `target/`, `.git/`, `.local/`
and `node_modules/` inside it are skipped, as in the workspace. The stage then:

1. appends `native-crates/<mod-id>/<crate-id>` to the root `[workspace] members`, copying the last
   member's formatting;
2. adds `<package> = { path = "<relative path>" }` to the dependent's `[dependencies]`;
3. adds the package to `Cargo.lock`: a `[[package]]` entry in Cargo's order and format, and its name
   in the dependent's `dependencies` list.

Manifests are edited with `toml_edit`, so comments and formatting elsewhere stay. Crates are staged
in mod id order, then manifest order.

The lock entries the stage writes cannot know which optional dependencies a registry crate's
features use, its default features included: `hashbrown = "0.15"` uses `allocator-api2` and
`equivalent`, which a workspace may lock for other crates while it uses hashbrown without its
defaults. So `apply` and `check` then run `cargo update --workspace` on a scratch copy of the stage
(offline from Cargo's cache first, else from the registry index; no crate is downloaded). It
re-resolves only the workspace members and keeps every locked version, adding the dependency edges
the features need. The stage gets that lock. A registry package it adds or changes (name, version,
source or checksum) is refused with `E_CRATE_DEPENDENCY`, naming it, before anything is staged: no
registry package is ever added or changed, so the stage builds with `--locked`.

A bundle crate may depend on registry packages already in the workspace's `Cargo.lock`, and on
other explicitly declared crates from the same bundle. Registry dependencies remain a version
string or a table using only `version`, `features`, `default-features` and `optional`; the highest
matching locked version is used. Horizon's lock file remains the registry allow list.

A same-bundle dependency uses a path table in `[dependencies]`, `[dev-dependencies]` or
`[build-dependencies]`:

```toml
[dependencies]
math_api = { path = "../../shared/math", package = "utility-kernel", version = "0.1.0" }
```

The path is relative to the original source crate root. It must resolve inside the bundle to the
exact root of another `[[crate]]` declaration, including when that crate is declared later or its
source root is nested. Parent segments are allowed only while they stay inside the bundle. Absolute,
drive, UNC and backslash paths, symlinks in any traversed component, missing/undeclared folders,
self-dependencies and cross-mod/game paths fail with `E_CRATE_DEPENDENCY` or `E_INVALID_PATH`.
The optional `package` must equal the target package name; without it, the dependency key must equal
that package name. An optional `version` requirement must match the target's literal package version.
`path`, `package`, `version`, `features`, `default-features` and `optional` are the only accepted
keys, and each value's TOML type is checked.

The plan stores the dependency table, alias key and target crate ID. Staging verifies the original
bundle hashes and the exact correspondence between path entries and planned edges before rewriting
only the `path` value to `../<target-crate-id>`. Alias, package, version, feature settings and
comments remain. `Cargo.lock` records the target package name as a dependency edge; no registry
package may be added or changed. Cycles are rejected deterministically across normal, build and dev
edges, including optional inactive edges, with an `E_CRATE_DEPENDENCY` chain. This conservative rule
keeps bundle build order unambiguous.

The game glue stays in helpers and support modules, and reusable heavy logic can remain in its own
bundle crate.

The crate's own data lives inside its folder and is copied with it:
`include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/data/table.json"))`.

Rejected, with `E_CRATE`:

- a crate in a direct `.toml` mod (crates need a bundle directory);
- a folder without `Cargo.toml`, or a `path` inside the bundle's `data/`;
- a `Cargo.lock` in the crate;
- `[workspace]`, `[target.*]`, `[patch]`, `[replace]`, `[profile]` or any other table outside
  `package`, `lib`, `bin`, `test`, `example`, `bench`, `features`, `lints`, `badges` and the three
  dependency tables;
- `package.workspace`, a non-literal `package.version`, and `package.build` or target `path`s that
  leave the crate;
- a package name (with `-` and `_` treated alike) that is already a workspace member, any package in
  `Cargo.lock`, another mod's crate, or a dependency name of the dependent;
- a `dependent` that is not a workspace member package. The message lists the members.

Dependency problems are `E_CRATE_DEPENDENCY`, for example
`boost needs rand ^0.8, which isn't in Horizon's Cargo.lock`. Declared crate-root paths cannot use
`..`; sibling dependency paths may use parent segments only within the bundle. Symlinked crate-root
components, dependency-path components, or files are rejected.

Cargo rebuilds a crate only when its sources or dependencies change. A bundle crate depends on
locked registry crates and declared siblings, so editing the mod's helpers or toggling another mod
leaves it built. Its staged files keep their mtimes, which the integration test checks. The game
crate still recompiles, but it no longer contains the crate's code.

## Data

Every file under `<bundle>/data/` (of any type, with nothing skipped) is staged to
`native-data/<mod-id>/` at the workspace root. Helpers and support modules in a game crate read it
relative to that crate. From `crates/horizon-client`:

```rust
include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../native-data/boost/greeting.txt"))
```

Horizon wraps this as `native_data!("<mod-id>", "path")`. A support module cannot call
`include_bytes!` directly, because modules must be self-contained. A `data` entry that is a file or a
symlink, or a symlink inside it, is `E_INVALID_PATH`. Data in direct `.toml` mods is not looked up.

## Post-build steps

```toml
[[post_build]]
crate = "engine"   # a [[crate]] id of the same mod
bin = "bake"       # a bin of that crate
args = ["--quick"] # optional, plain strings
```

A bin is a `[[bin]] name`, `src/bin/<name>.rs`, `src/bin/<name>/main.rs`, or the package name
when `src/main.rs` exists. Every stage contains `native-post-build.json`, which is `[]` when no mod
has a step:

```json
[
  {
    "mod": "boost",
    "package": "boost-engine",
    "bin": "bake",
    "args": ["--quick"]
  }
]
```

Entries are in mod id order, then manifest order. The launcher builds and runs them after the game
compiles. `E_POST_BUILD` covers a `crate` that is not one of the mod's crates, an invalid bin name,
a bin the crate does not have (the message lists the crate's bins), and a bin whose file name
(ignoring case) is already a workspace bin or another crate's post-build bin: every bin is built
into the same target folder, so one would replace the other.

## Reserved paths

The stage owns `native-crates/`, `native-data/` and `native-post-build.json`. A workspace that
already has any of them fails with `E_INVALID_PATH`. The plan records each crate's and data
folder's file fingerprints, and staging checks them again before it publishes.

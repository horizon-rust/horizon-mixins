# Feasibility, acceptance and measurements

Implemented and exercised on 2026-10-28 with stable rustc 1.95.0
(`59807616e`), Linux x86_64. The Cargo workspace and fixtures commit their
lockfiles; the fixture also carries its toolchain pin into staging.

The phase-0 prototype was compiled before implementation of the engine. A
private `Entity.velocity` field is modified by a local helper taking
`&mut Entity`, called with an explicit reborrow of `self`; the separate driver
crate verifies two successive results. The positive two-crate fixture passed.
A separate mod crate trying to access that field failed Rust privacy checking.
A game→mod→game fixture failed Cargo's cyclic dependency check. These are real
compiler/Cargo failures asserted by `tools/feasibility.py`, not simulated errors.

The resulting binding design compiles one ordinary helper function directly
inside the target's lexical module/function scope. Every context value comes
from a declared target parameter binding. Concrete helper argument types,
return types, private access and reborrows are checked by staged Cargo tests.
The MVP adds no dependencies and requires no cross-crate private visibility.
See [mixin-api.md](mixin-api.md) for syntax and limits.

The main fixture has a game crate and a driver crate, plus two independent mod
bundles. Its four runtime tests prove private-field HEAD ordering, function-call
result/exactly-once evaluation, method receiver/argument evaluation, and
OVERWRITE. The engine integration suite adds source/plan snapshots, manifest
and selector failures, cfg/macros, ambiguity, conflicts, borrowing/return-type
compilation failures, concurrent checks, immutable originals and preserved
mtimes. The initial MVP passed all 16 baseline integration tests. Linux CI runs rustfmt, clippy with
warnings denied, phase-0 compilation and the integration suite.

A first four-scenario fixture measurement produced:

| Scenario | Transform/stage wall | Cargo compile | Cargo build + tests |
|---|---:|---:|---:|
| First modded build | 0.056 s | 0.13 s | 0.171 s |
| Unchanged run | 0.053 s | 0.00 s | 0.042 s |
| One helper changed | 0.051 s | 0.09 s | 0.129 s |
| Shared game source changed | 0.053 s | 0.08 s | 0.128 s |

Measured platform: `Linux-7.3.0-2-cachyos-x86_64-with-glibc2.44`.
These tiny builds establish cache reuse; they do not predict full-game compile
time. `tools/benchmark.py` stores raw Cargo logs/reports under its fresh output
directory. The Cargo compile column is Cargo's reported test-profile duration;
the wall column includes executing tests and doc tests.

The Horizon experiment exports the complete `horizon-core` crate from local
Git commit `028f60881678a9a0e3e198f693a82858fa475c76`. Its crate sources remain
unchanged before transformation. The experiment replaces only the exported
root workspace manifest to isolate that crate, reduces the exported lockfile
to its existing exact `libm = 0.2.16` dependency, adds an export revision stamp,
and adds two test probes. The local Horizon checkout is never edited.

Two separately declared HEAD hooks target `horizon_core::jmath::atan`. Each
panics at a different sentinel input, and the probe catches the exact messages
to prove both hooks execute. A normal input retains the original bitwise result.
All upstream library and integration tests pass: 106 library tests, 8 existing
integration tests, 2 added probes, and 2 deliberately ignored upstream
microbenchmarks. No Minecraft resources are needed.

The first upstream run measured 0.377 s transformation/staging, 2.71 s Cargo
compilation and 12.324 s Cargo-plus-tests. An identical run preserved the stage
and reported 0.00 s Cargo compilation; its 9.606 s wall time is primarily the
upstream tests, rather than recompilation. The baseline untransformed library
test run took 11.884 s including compilation and execution (2.33 s reported compilation). Fresh repetitions
will differ. Run `tools/horizon_prototype.py` with a fresh `--out` to generate
raw baseline/Cargo logs and a report on the current machine.

This validates the isolated complete foundation crate, not the full Horizon
game, graphics/audio stack, prepared assets, launcher, or production gameplay
compatibility. Additional injection kinds, semantic resolution, cross-platform atomic
replacement and executable publication/caching remain future work.


Version 0.2.0 adds five advanced features, with the semantic specification in
[advanced-injections.md](advanced-injections.md). A separate stable-Rust spike
compiled ControlFlow cancellation, owned FnOnce chains and temporary-preserving
value calls before implementation. The new dependency-free two-crate fixture
has 38 injections across two mods and eight runtime tests. It covers cancellation
precedence, all supported return exits, owned/borrowed results, private method
receivers, argument/constant chains, skipping operations and combined phases.
Further integration cases trace temporary/local destruction, nested returns,
compiler rejection of double continuation calls and moved values, wrong original
argument types, failed signatures/indices/sites, unchanged mtimes and source,
and deterministic schema-2 plans/source snapshots. The Sponge feature map
tracks remaining capabilities rather than implying Java API compatibility.


The final 0.2.0 validation passes 27 engine integration tests, including the 16
baseline cases; those compile the four baseline and eight advanced fixture
runtime tests plus independent scope/borrow/error probes. Rustfmt and clippy
with warnings denied pass. Selector regressions also check that spaces inside
string literals and identifier boundaries cannot silently select a different
receiver expression.

An advanced-fixture measurement recorded 0.090 s transformation/staging,
0.20 s Cargo compilation and 0.248 s Cargo-plus-tests on its first build. An
unchanged run retained the stage and recorded 0.087 s transformation/staging,
0.00 s compilation and 0.046 s Cargo-plus-tests. Raw local reports are under
`target/advanced-v020-{first,unchanged}.json`; these values describe this tiny
fixture on this machine.

The existing pinned Horizon HEAD prototype was rerun with 0.2.0 and again
passed 116 library/integration/probe tests with two ignored upstream benchmarks.
Its first staged build reported 2.67 s compilation, and the unchanged run 0.00 s.
This regression confirms the existing isolated-crate integration; the new five
features are validated on the advanced fixture, not claimed compatible with all
Horizon functions or a complete game build.

Version 0.3.0 adds lexical capture, call-bounded slices, reusable field/method
bridges, MODIFY_ARGS and MODIFY_VARIABLE. See
[scopes-and-bridges.md](scopes-and-bridges.md) for their contracts. A separate
two-crate fixture compiles 25 injections and nine bridges from two independent
mods, with eight runtime tests. The 44 engine integration tests include all
earlier cases and positive/negative capture scope, declaration shadowing,
inferred types, owned moves, mutable borrows, native privacy, associated/trait
invokers, bridge name collisions, slice bounds, tuple composition, scalar loads
and owned STORE destruction. Plan/source snapshots use schema 3. Rustfmt,
clippy with warnings denied and the phase-0 privacy/cycle checks pass.

The scoped fixture measured:

| Scenario | Transform/stage wall | Cargo compile | Cargo build + tests |
|---|---:|---:|---:|
| First modded build | 0.085 s | 0.19 s | 0.242 s |
| Unchanged run | 0.082 s | 0.00 s | 0.047 s |

The unchanged run retained the stage. Raw reports are under
`target/scoped-v030-{first,unchanged}.json`. Measurements are specific to this
tiny fixture and machine, not estimates for the full game.

The 0.3.0 pinned Horizon HEAD regression again passed 116 tests with two ignored
upstream benchmarks. First-stage transformation took 0.374 s, compilation
2.60 s and Cargo-plus-tests 12.369 s. Its unchanged run retained the stage,
reported 0.02 s compilation and took 9.574 s including tests; baseline library
compilation/tests took 12.037 s. Raw logs/reports are under
`target/horizon-prototype-v030`. This verifies existing isolated `horizon-core`
integration; the five new features are exercised by the scoped fixture. The
upstream checkout and Minecraft assets were not modified or used.

Version 0.4.0 adds lifetime-only impl method identities, HEAD over preserved
closure/`?` bodies and self-contained support modules. Its 48 integration tests
include two-mod lifetime HEAD order, error propagation, exactly-once Drop,
cancellation, module conflicts, scope restrictions and source/plan snapshots.
Rustfmt, clippy with warnings denied, and privacy/cycle feasibility checks pass.
The separate Veinminer 0.2.0 port now consumes a real TOML bundle: three HEAD
hooks and one runtime module. The complete pinned Horizon game binary
type-checks, and its 13 embedded mining tests pass. A canonical AST audit
verifies all other original syntax is preserved. First native check/test Cargo
wall times were 25.882 s / 49.394 s; full-workspace plan plus apply cost about
26 s including two planner passes. These measurements cover source builds and
tests without game assets; live gameplay remains unverified.

Version 0.5.0 implements the [effects and state contracts](effects-and-state.md).
Before implementation, `tools/effects-feasibility.rs` compiled typed value/after
adapters and compared original/result/argument Drop order, including cancellation,
then checked once-only expression evaluation and per-instance ownership. Its two
Rust tests pass through the phase-0 script. Existing privacy and dependency-cycle
failures still occur as expected.

The effects workspace contains twelve injections and two state declarations
across independent mods. Five runtime tests compile together. Engine integration
cases additionally check move/type/alias failures, original count/ordinal errors,
post-assignment visibility, cancellation/RETURN capture visibility, unsupported
layouts/derives/patterns/aliases, constructor coverage and overwrite conflicts,
state-only bundles, derived Default, Drop lifecycle and namespaced collisions.
The 64 integration tests pass, including byte-identical plan/source snapshots,
immutable original files and unchanged stage mtimes. Rustfmt and clippy with
warnings denied pass. Plans and package version are schema 5 / 0.5.0.

Measurements on the same Linux/rustc machine, fresh Cargo target for the effects
fixture (includes its existing serde dependencies):

| Scenario | Transform/stage wall | Cargo compile | Cargo build + tests |
|---|---:|---:|---:|
| First effects/state build | 0.063 s | 2.09 s | 2.133 s |
| Unchanged run | 0.065 s | 0.01 s | 0.048 s |

Raw reports are generated under `target/effects-measure-v050/`. These are tiny
fixture measurements; no full-game rebuild speed claim follows from them.
The pinned isolated horizon-core experiment was also repeated under
`target/horizon-prototype-v050/`: the original upstream tests and two added HEAD
probes pass. First modded transform/stage: 0.369 s; Cargo compile: 2.53 s; Cargo
plus tests: 12.169 s. Unchanged: 0.363 s transform/stage, 0.00 s compile,
9.706 s Cargo plus tests. It still uses the same committed Horizon pin and no
Minecraft assets or upstream source edits.

HEAD-only macro preservation builds on contributor PR #2 and now covers both
ordinary and cancellable HEAD hooks. The 72 integration tests compile `format!`,
`matches!`, list/repeat `vec!`, custom macro early returns, cancellation and Drop
behavior. Planning and staging cancellable HEAD hooks on the actual
`Game::enter_guest` and `Mob::damage_with` methods also succeed against Horizon
commit `3c9bb38e88f5b95ce42a139bc600d3e9c61a2175`; removing the generated statements
reproduces each complete file after canonical formatting. This source audit
does not claim a full-game compilation or gameplay run.

Version 0.6.0 implements [named hooks, bundle crates, data and post-build steps](crates-data-and-named-hooks.md).
The `fixtures/native` workspace has a two-hook catalog. The `boost` bundle has a named HEAD hook, a
crate (`boost-engine`, depending on the locked `serde_json`, with its own `data/` and a `bake` bin),
a binary data file and a post-build step. The staged workspace passes `cargo test --workspace --locked
--offline`, so the edited `Cargo.lock` is accepted unchanged. Nine integration tests cover the
compiled path, unchanged bytes and mtimes (including the crate's files across a helper edit), two
mods' crates, and every new diagnostic. Applying a crate-carrying bundle to a Horizon `main` export
(with the #3871 catalog) also produced a stage that `cargo metadata --locked` accepts. The 0.6.0
release had 89 integration tests and schema-6 plans. This checkout adds same-bundle dependency edges
in schema 7; the Cargo package version remains unchanged until release.

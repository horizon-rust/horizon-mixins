# Roadmap and acceptance matrix

Phases 0–4 are implemented. Phase 5 is verified for an isolated complete
`horizon-core` crate at `028f60881678a9a0e3e198f693a82858fa475c76`.
Full game and launcher integration remain future work.

| Phase | Result |
|---|---|
| 0 — Feasibility | Two-crate compilation fixture proves local bridge/private fields/reborrowing; private cross-crate access and dependency cycles fail as expected |
| 1 — Parsing/index | Three-crate CLI workspace; strict TOML; inline/external/path modules; free/inherent/trait functions; missing/ambiguous/cfg/macro diagnostics |
| 2 — Planning | Canonical JSON; original-site fingerprints; explicit counts/ordinals; deterministic priority/ID order; overwrite conflicts before mutation |
| 3 — Transform | HEAD, BEFORE_CALL and OVERWRITE; source/plan snapshots; compiled behavior, borrowing and return-type tests |
| 4 — Staging/Cargo | Immutable originals; atomic Linux publication; unchanged mtimes; stable locked target directories; concurrency tests and four-scenario benchmark |
| 5 — Upstream prototype | Isolated pinned horizon-core export, two HEAD hooks in jmath::atan, upstream tests and hook probes; no assets/upstream edits |
| 6a — Advanced value/control features | Implemented: cancellable HEAD, RETURN, MODIFY_ARG, MODIFY_CONSTANT and composable WRAP_OPERATION with typed contracts, original-site planning and compiled tests |
| 6b — Scoped features and bridges | Implemented: lexical local capture, same-block call-bounded slices, GET/GET_MUT/SET/INVOKE APIs, MODIFY_ARGS and MODIFY_VARIABLE STORE/scalar LOAD |
| 6c — Native consumer | Implemented: lifetime impl method IDs, HEAD over preserved closure/`?` bodies, support modules and a real TOML Veinminer bundle; complete game binary compilation and AST audit |
| 6d — Effects and state | Implemented: expression results, AFTER_CALL/AFTER_ASSIGN, interior cancellation, explicit per-instance fields/constructors and serde old-data defaults |
| 6e — Bundle crates, data and named hooks | Implemented (0.6.0): `native-hooks.json` named hooks with versions, bundle crates as workspace members with locked registry dependencies, `data/` staging, `native-post-build.json`; see [crates, data and named hooks](crates-data-and-named-hooks.md) |
| 6f — Same-bundle crate dependencies | Implemented: explicitly declared sibling path edges with forward references, nested roots, path/type validation, deterministic cycle rejection and registry lock preservation; plans use schema 7; see [crates, data and named hooks](crates-data-and-named-hooks.md) |
| 6g — Remaining parity | Future: REDIRECT, broader sites/receivers/patterns, arbitrary members/traits, optional groups and semantic matching; see [feature map](spongepowered-feature-gaps.md) |

`cargo test --workspace --locked` checks two independent mods, same-site priority
and ID tie order, original selectors unaffected by injected calls, call results
and exactly-once arguments/receiver evaluation, overwrite success/conflicts,
source/plan snapshots, discovery order independence, repeated bytes/mtimes,
immutable originals, removed files, concurrent Cargo checks, and stale source.
It also checks missing/ambiguous targets, wrong counts/ordinals, duplicate IDs,
malformed manifests/helpers, invalid paths, cfg/macros, unproven temporal scopes,
parameter shadowing and compiler-diagnosed helper borrow/return failures.

Linux CI runs the phase-0 script, rustfmt, clippy with warnings denied, and the
integration suite. Expand atomic staging and compilation CI to Windows/macOS
after implementing their directory-publication strategy. Launcher work requires
a separate change; it is not part of this MVP.

The advanced fixture adds 38 declarations across two mods and eight runtime
tests. Integration tests cover cancellation precedence, return exits and Drop,
argument/constant value chains, FnOnce enforcement, method/self reborrowing,
combined phases, signature conflicts, invalid sites and deterministic schema-7
plans. See [advanced-injections.md](advanced-injections.md) for supported limits.

The scoped fixture adds 25 injections, nine bridges and eight runtime tests
across two mods. The engine integration tests include lexical declaration
identity and shadowing, borrowed/owned capture errors, bridge privacy and mutable
borrows, slice boundaries/counts, argument tuple composition, STORE destruction,
LOAD-before-STORE order, bridge-only bundles, and deterministic schema-7 plans
and source. See [scopes-and-bridges.md](scopes-and-bridges.md).

The effects fixture adds two independently declared states and twelve hooks,
with five runtime tests and source/plan snapshots. Additional compiled cases
cover owned expression results, noncancellable result destruction, post-store
RETURN captures, state-only bundles, derived Default and Drop lifecycles.
See [effects-and-state.md](effects-and-state.md). The full engine suite has
64 integration tests; the independent feasibility spike runs two additional
Rust tests before the suite.

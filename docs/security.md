# Security and supported boundaries

Native helper source, dependencies, build scripts and proc macros can execute
with user privileges. They are not sandboxed. `check` requires
`--trust-native-mods` and reports an explicit diagnostic without that opt-in.
Planning/staging does not compile the source or run build scripts; it uses
Cargo metadata and Rust syntax parsing. Review the workspace's Cargo configuration
and toolchain as part of trusting the native workspace.

Cargo.lock is required, dependency resolution/builds are locked and offline,
and source files/toolchain are fingerprinted before rewriting. Horizon bundles
can pin an exact commit and original function fingerprint. Source export revision
stamps are trusted provenance metadata, not signed attestations. Helper path
traversal, symlink escapes, source symlinks, external path dependencies and
output paths overlapping originals/mods are rejected. Staging only replaces
its own marked outputs, atomically on Linux.

These checks prevent accidental source escape and mutation under a cooperative
local filesystem. They are not protection against a hostile local process racing
filesystem checks, malicious trusted build scripts, arbitrary helper behavior or
manual edits through hard-linked prior-stage files. Do not modify a stage while
`check` builds it. An OS sandbox and signed/verified mod bundles remain future work.

Rust privacy, ownership, borrowing, orphan rules and trait resolution still
apply. Helpers compile inside the target crate with explicit context. `syn`
provides syntax, not macro expansion or semantic resolution. Unproven source
contexts are rejected; successful compilation does not establish gameplay safety.

No game binaries, Minecraft assets, decompiled Java or prepared resources belong
in this repository. The Horizon prototype exports only committed Rust crate
source. License: MIT; see the repository LICENSE.

Per-instance state changes the staged struct's Rust layout. repr/FFI and openly
constructible public structs are rejected; constructor literals and initializers
are explicit and fingerprinted. State/serialization code has the same native
privileges as helpers. Serde defaults support old records, but custom game save
systems and schema migrations require explicit integration; see
[effects and state](effects-and-state.md).

Bundle crates (0.6.0) are native code with the same privileges, including their build scripts.
Registry dependencies must already be pinned in the workspace's `Cargo.lock`. A path dependency is
accepted only when it resolves inside the same bundle to another declared crate root; the planner
rejects escapes, symlinks, undeclared targets, self-edges and cycles across normal/build/dev edges,
including inactive optional dependencies. Git/workspace dependencies and `[workspace]`, `[patch]`,
`[replace]` or `[target]` tables remain unsupported. Target paths must stay inside each crate.
`[[post_build]]` steps are recorded, never executed, by horizon-mixins; the launcher that runs them
has to trust them as it trusts the mod.

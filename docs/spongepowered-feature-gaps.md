# SpongePowered Mixin feature map and remaining work

Status: Horizon Mixins 0.6.0 with schema-7 same-bundle crate edges. This is a
capability comparison and design backlog, not Java annotation, bytecode or mod compatibility. The implementation uses Rust
source before Cargo compilation; Sponge Mixin integrates Java bytecode with the
runtime classloading pipeline. [Sponge overview](https://github.com/SpongePowered/Mixin/blob/master/README.MD).

The source links below point to upstream documentation/annotation definitions.
Rust design notes are this project's proposals, not Sponge guarantees. Missing
features are not accepted TOML kinds. Unknown fields/kinds still fail validation.

## Implemented counterparts

| Sponge capability | Horizon counterpart | Supported boundary |
|---|---|---|
| HEAD callback | HEAD | Explicit typed target-parameter context |
| Cancellable callback | HEAD, BEFORE_CALL, AFTER_CALL, AFTER_ASSIGN with `cancellable = true` | ControlFlow returns from the target; whole-expression/statement proofs for interior sites |
| Return callbacks/result replacement | RETURN | All normal explicit/implicit exits, including cancellation; no individual return selectors, async, `?` or closure exits |
| INVOKE callback | BEFORE_CALL | Exact syntax; only whole statements, plain let initializers and block/function tails |
| Overwrite | OVERWRITE | Entire body; exclusive against every other injection |
| ModifyArg | MODIFY_ARG | One selected argument; explicit index and same input/output type |
| ModifyConstant | MODIFY_CONSTANT | Exact runtime numeric/bool/char/string literals; no compile-time expressions or implicit comparison constants |
| ModifyArgs | MODIFY_ARGS | Typed tuple of all explicit call arguments; separate borrowed receiver and explicit owned/unit result |
| ModifyVariable | MODIFY_VARIABLE | Plain local/incoming parameter identities; initializer/simple assignment STORE and primitive scalar value LOAD |
| Local capture | Explicit `$local`, `$local_ref`, `$local_mut` bindings | Plain let declarations with ordinals and lexical visibility checks; Cargo checks moves/borrows; no surrogates |
| Slices | Optional call-selected from/to bounds | Pristine expression interval, same lexical block; inclusive start/exclusive end; explicit boundary counts/ordinals |
| Accessor / Invoker | GET, GET_MUT, SET, INVOKE bridges | Public namespaced inherent methods on owned nongeneric structs; fields and exact existing inherent/trait calls |
| Priorities, ordinals and count validation | priority, ordinal, require | Deterministic ID tie-breaks; require is an exact original match count here |
| After INVOKE / INVOKE_ASSIGN | AFTER_CALL / AFTER_ASSIGN | Owned/unit result borrows before consumption, or post-statement lexical captures after the store/temporary drops |
| MixinExtras ModifyExpressionValue | MODIFY_EXPRESSION_VALUE | Exact syntax expression selectors and explicit value types; runtime values and scalar reads, excluding places |
| Unique instance fields | Namespaced state fields/getters | Owned named nongeneric structs, explicit constructors and derived serde defaults; custom saves remain explicit |
| MixinExtras WrapOperation | WRAP_OPERATION | Chained FnOnce continuations; module-level free calls or borrowed parameter receivers; owned result/unit |

See [advanced-injections.md](advanced-injections.md) and
[scopes-and-bridges.md](scopes-and-bridges.md) for the actual TOML API,
composition phases and compilation-tested limits. Sponge callbacks offer broader
site selection and cancellation. Its `require` is a minimum rather than our exact
match count. [Callback documentation](https://github.com/SpongePowered/Mixin/wiki/Advanced-Mixin-Usage---Callback-Injectors).

## Missing injection and selection features

| Feature | What Sponge provides | Rust design needed |
|---|---|---|
| TAIL and individual RETURN selection | Final bytecode return, or a chosen return ordinal | Define Rust source exit identities; TAIL is not an alias for every return and must not silently omit early exits |
| FIELD, NEW, JUMP sites | Field accesses, allocation/construction and branches | Distinguish place expressions from values, struct literals from constructors, and branches from enclosing expressions |
| Redirect | Replace calls, field/array operations, object creation and selected type checks | Independent conflict rules; preserve places, indexing evaluation, borrow order, receiver effects and replacement types |
| Broader argument/variable support | Broad bytecode-level argument/local selection | Extend proven Rust cases beyond borrowed parameter receivers, plain lets, simple stores and scalar loads; preserve borrowing, moves and places |
| Pattern capture / surrogates | Supply locals and alternative compatible handlers | Conditional/destructured pattern identities, broader return contexts and explicit fallback rules; plain let capture is implemented |
| Broader slice landmarks | General injection-point slice bounds | Current bounds are calls in the same lexical block; branches, arbitrary expressions and cross-scope bounds need separate contracts |
| Shifted, custom and multi-target selectors | Relative instruction offsets, custom injection points, wildcard/multiple method targets | Source-level equivalents with explicit cardinality, deterministic expansion and ambiguity checks |

References: [injection point reference](https://github.com/SpongePowered/Mixin/wiki/Injection-Point-Reference),
[Redirect](https://github.com/SpongePowered/Mixin/blob/master/src/main/java/org/spongepowered/asm/mixin/injection/Redirect.java),
[ModifyArgs](https://github.com/SpongePowered/Mixin/blob/master/src/main/java/org/spongepowered/asm/mixin/injection/ModifyArgs.java),
[ModifyVariable](https://github.com/SpongePowered/Mixin/blob/master/src/main/java/org/spongepowered/asm/mixin/injection/ModifyVariable.java),
[local capture](https://github.com/SpongePowered/Mixin/blob/master/src/main/java/org/spongepowered/asm/mixin/injection/Inject.java),
[Slice](https://github.com/SpongePowered/Mixin/blob/master/src/main/java/org/spongepowered/asm/mixin/injection/Slice.java),
[custom injection points](https://github.com/SpongePowered/Mixin/blob/master/src/main/java/org/spongepowered/asm/mixin/injection/InjectionPoint.java).

AFTER_CALL now compiles owned/unit adapters and Drop/cancellation cases; borrowed
results are rejected. AFTER_ASSIGN can capture stored borrowed results through
ordinary Rust types. The local-variable fixture demonstrates that two bindings
with the same name cannot be confused.
A redirect prototype must reject competing replacements before staging.
None should infer method identity from a bare method name.

## Missing member and type transformations

| Feature | Current gap | Proposed Rust boundary |
|---|---|---|
| Shadow | No declarative placeholders for existing fields/methods | Explicit member contracts validated in the target module; local helpers already have ordinary same-crate privacy |
| Broader accessor / invoker support | Nongeneric owned structs only; unsafe/async/const/generic signatures and unresolved associated projections rejected | Extend signature/owner resolution with compiled privacy/lifetime examples; current bridges add no dependency edges |
| Broader fields / methods and Unique | Namespaced owned instance state, support modules and generated getters/bridges are implemented; arbitrary members and generic state owners are missing | Extend explicit constructor, layout, derive and serialization contracts with compiled cases |
| Interface / soft-implements mixins | No generated trait implementations or duck-interface adapters | Rust coherence/orphan rules, trait dependencies, associated items and existing impl conflicts must be checked by Cargo |
| Intrinsic proxies / superclass behavior | No equivalent proxy mechanism | Define explicit delegation to original functions; Rust has no direct Java superclass model |
| Final / Mutable controls | No Java-style field finality transformation | Rust mutability and interior-mutability rules remain enforced; an annotation must never promise to bypass borrowing |

References: [Shadow](https://github.com/SpongePowered/Mixin/blob/master/src/main/java/org/spongepowered/asm/mixin/Shadow.java),
[accessor/invoker generation](https://github.com/SpongePowered/Mixin/blob/master/src/main/java/org/spongepowered/asm/mixin/gen/AccessorInfo.java),
[Unique](https://github.com/SpongePowered/Mixin/blob/master/src/main/java/org/spongepowered/asm/mixin/Unique.java),
[member merging](https://github.com/SpongePowered/Mixin/blob/master/src/main/java/org/spongepowered/asm/mixin/transformer/MixinApplicatorStandard.java),
[interfaces and architecture](https://github.com/SpongePowered/Mixin/wiki/Introduction-to-Mixins---Understanding-Mixin-Architecture),
[Final](https://github.com/SpongePowered/Mixin/blob/master/src/main/java/org/spongepowered/asm/mixin/Final.java),
[Mutable](https://github.com/SpongePowered/Mixin/blob/master/src/main/java/org/spongepowered/asm/mixin/Mutable.java).

Instance state uses explicit constructor coverage, rejects repr/open external
construction, and supplies serde defaults for missing fields. Custom saves and
entity-ID side tables remain explicit mod code. See [state contracts](effects-and-state.md).

## Missing configuration, tooling and compatibility features

| Feature | Current gap / relevant direction |
|---|---|
| Conditional configs and plugins | No client/server config selection, programmatic shouldApplyMixin or companion plugins. Prefer deterministic declarative feature/side conditions first. |
| Optional/virtual targets and fallback groups | Missing targets always fail. Optionality and alternatives would need explicit declarations and group-level cardinality checks; never silently skip mandatory hooks. |
| Group min/max, expect/allow and constraints | We have exact require per selector, not group-level alternatives or separate debug expectations. |
| Refmaps / obfuscation mapping | Exact pinned Rust paths only. Cross-revision adapters may be useful; Java obfuscation mapping is not presently needed. |
| Annotation ergonomics | TOML only; Rust attributes could become syntax sugar over the same validated IR. |
| Expanded diagnostics and development tools | We have JSON diagnostics, plans, source snapshots and Cargo logs. No bytecode verifier/export, IDE plugin, interactive site explorer or hot-reload workflow. |
| Runtime loader integration | No classloader analogue, launcher integration, binary patcher or running-game hot replacement. Native Rust changes require compilation. |

References: [configuration](https://github.com/SpongePowered/Mixin/blob/master/src/main/java/org/spongepowered/asm/mixin/transformer/MixinConfig.java),
[configuration plugins](https://github.com/SpongePowered/Mixin/blob/master/src/main/java/org/spongepowered/asm/mixin/extensibility/IMixinConfigPlugin.java),
[Group](https://github.com/SpongePowered/Mixin/blob/master/src/main/java/org/spongepowered/asm/mixin/injection/Group.java),
[reference maps](https://github.com/SpongePowered/Mixin/blob/master/src/main/java/org/spongepowered/asm/mixin/refmap/ReferenceMapper.java),
[Sponge tooling entry point](https://github.com/SpongePowered/Mixin/wiki).

## MixinExtras is a separate comparison

Operation wrapping and expression-value modifiers come from the separate
MixinExtras project. Our WRAP_OPERATION follows the general chaining model but
uses typed Rust FnOnce and our own manifests/order rules; it does not load Java
MixinExtras mods. [WrapOperation documentation](https://github.com/LlamaLad7/MixinExtras/wiki/WrapOperation).

`MODIFY_EXPRESSION_VALUE` now selects exact original syntax and preserves
once-only evaluation and short-circuit behavior. Place expressions, implicit
borrowed owned reads, compile-time values and unproven control flow are excluded.
See [effects and state](effects-and-state.md) and upstream
[ModifyExpressionValue documentation](https://github.com/LlamaLad7/MixinExtras/wiki/ModifyExpressionValue).

## Suggested next feature work

1. Exercise the supported operations on real horizon-world state in an isolated
   pinned checkout and widen receiver/context support based on compiled cases.
2. Widen borrowed-result adapters only with lifetime and destruction-order proofs.
3. Add optional declarative compatibility groups and extend existing receiver,
   pattern and bridge support only for compiled use cases.
4. Consider redirects and broader member/trait transformations with independent
   semantics and conflict tests.

Launcher/build-cache work is tracked separately in [roadmap.md](roadmap.md).
Feature parity should be judged by tested modding use cases, not by copying
annotation names onto syntax transformations with different semantics.

Version 0.4.0 also supports lifetime-bearing impls and HEAD over preserved
closure/`?` bodies; see [native consumer contracts](boundary-head-and-modules.md).
The newer state extension adds explicit constructor rewriting separately. Other
body-rewriting kinds retain their existing safety limits.

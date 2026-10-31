use crate::{
    Diagnostic, Result, bridges,
    diagnostic::io,
    helpers::{self, Contract},
    index::{self, Function, Index, tokens},
    manifest::{self, Binding, Kind},
    scopes::{self, Scopes},
    sites::{self, Sites},
    slices, source,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, process::Command};
use syn::{
    Expr, Item,
    visit::{self, Visit},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub schema: u32,
    pub transformer: String,
    pub rustc: String,
    pub inputs: BTreeMap<String, String>,
    pub injections: Vec<PlannedInjection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bridges: Vec<bridges::PlannedBridge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<PlannedModule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<crate::state::PlannedState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crates: Vec<crate::native::PlannedCrate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<crate::native::PlannedData>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub post_build: Vec<crate::native::PlannedPostBuild>,
}
/// The current plan schema: 6 adds named hooks, bundle crates, data and post-build steps.
pub const SCHEMA: u32 = 6;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedModule {
    pub mod_id: String,
    pub mod_version: String,
    pub module_id: String,
    pub file: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "manifest::ModuleVisibility::is_crate")]
    pub visibility: manifest::ModuleVisibility,
    pub fingerprint: String,
    pub source: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedInjection {
    pub mod_id: String,
    pub mod_version: String,
    pub injection_id: String,
    pub priority: i32,
    pub target: String,
    /// The catalog hook the target was resolved from, when the manifest named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_version: Option<u32>,
    pub file: String,
    pub node: Vec<usize>,
    pub function_fingerprint: String,
    pub kind: Kind,
    pub cancellable: bool,
    pub argument: Option<usize>,
    pub constant_site: Option<usize>,
    pub contract: Contract,
    /// Preorder call number in the pristine body, never in an already rewritten tree.
    pub call_site: Option<usize>,
    pub site_fingerprint: Option<String>,
    pub selector: Option<String>,
    pub ordinal: Option<usize>,
    pub require: Option<usize>,
    pub helper_source: String,
    pub bindings: Vec<Binding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub captures: Vec<scopes::Capture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression_site: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variable_site: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variable: Option<VariableIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slice: Option<slices::ResolvedSlice>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariableIdentity {
    pub local: scopes::Local,
    pub at: manifest::Access,
}

impl Plan {
    /// Validate externally deserialized IR before traversing any AST path.
    pub fn validate_structure(&self) -> Result<()> {
        let mut modules = std::collections::BTreeSet::new();
        for m in &self.modules {
            source::relative(Path::new(&m.file))?;
            syn::parse_str::<syn::Ident>(&m.name)
                .map_err(|e| Diagnostic::new("E_MODULE", e.to_string()))?;
            crate::modules::validate_payload(&m.source)?;
            if !modules.insert((&m.file, &m.name)) {
                return Err(Diagnostic::new(
                    "E_MODULE_CONFLICT",
                    "duplicate support module",
                ));
            }
        }
        for c in &self.crates {
            source::relative(Path::new(&c.path))?;
            source::relative(Path::new(&c.dependent_manifest))?;
            if !c.path.starts_with(&format!("{}/", crate::native::CRATES))
                || !c.files.contains_key("Cargo.toml")
            {
                return Err(Diagnostic::new("E_PLAN_SITE", "invalid planned crate"));
            }
            for name in c.files.keys() {
                source::relative(Path::new(name))?;
            }
        }
        for d in &self.data {
            for name in d.files.keys() {
                source::relative(Path::new(name))?;
            }
        }
        if self.post_build.iter().any(|p| {
            !self
                .crates
                .iter()
                .any(|c| c.mod_id == p.mod_id && c.crate_id == p.crate_id && c.package == p.package)
        }) {
            return Err(Diagnostic::new(
                "E_PLAN_SITE",
                "post-build step names no planned crate",
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for p in &self.injections {
            source::relative(Path::new(&p.file))?;
            if p.hook.is_some() != p.hook_version.is_some() {
                return Err(Diagnostic::new(
                    "E_PLAN_SITE",
                    "hook and hook_version go together",
                ));
            }
            let call_fields_valid = if p.kind.selects_site() {
                (if p.kind == Kind::ModifyConstant {
                    p.constant_site.is_some() && p.call_site.is_none() && p.variable_site.is_none()
                } else if p.kind == Kind::ModifyVariable {
                    p.variable_site.is_some() && p.call_site.is_none() && p.constant_site.is_none()
                } else if p.kind == Kind::ModifyExpressionValue {
                    p.call_site.is_none() && p.constant_site.is_none() && p.variable_site.is_none()
                } else {
                    p.call_site.is_some() && p.constant_site.is_none() && p.variable_site.is_none()
                }) && p.site_fingerprint.is_some()
                    && (if p.kind == Kind::ModifyVariable {
                        p.variable.is_some() && p.selector.is_none()
                    } else {
                        p.selector.as_ref().is_some_and(|s| !s.is_empty())
                    })
                    && p.expression_site.is_some()
                    && matches!((p.ordinal, p.require), (Some(o), Some(n)) if n > 0 && o < n)
            } else {
                p.call_site.is_none()
                    && p.variable_site.is_none()
                    && p.expression_site.is_none()
                    && p.slice.is_none()
                    && p.constant_site.is_none()
                    && p.site_fingerprint.is_none()
                    && p.selector.is_none()
                    && p.ordinal.is_none()
                    && p.require.is_none()
            } && (!p.cancellable
                || matches!(
                    p.kind,
                    Kind::Head | Kind::BeforeCall | Kind::AfterCall | Kind::AfterAssign
                ))
                && ((p.kind == Kind::ModifyArg) == p.argument.is_some())
                && ((p.kind == Kind::WrapOperation) == p.contract.operation.is_some())
                && ((p.kind == Kind::ModifyArgs) == p.contract.bundle.is_some())
                && ((p.kind == Kind::ModifyVariable) == p.variable.is_some())
                && (matches!(
                    p.kind,
                    Kind::Return
                        | Kind::ModifyArg
                        | Kind::ModifyConstant
                        | Kind::ModifyArgs
                        | Kind::ModifyVariable
                        | Kind::ModifyExpressionValue
                ) == p.contract.value.is_some())
                && ((p.kind == Kind::AfterCall) == p.contract.result.is_some());
            let metadata_valid = p.slice.as_ref().is_none_or(|s| {
                s.start < s.end && p.expression_site.is_some_and(|i| s.start <= i && i < s.end)
            }) && (p.kind != Kind::ModifyVariable
                || p.variable_site == p.expression_site);
            if !call_fields_valid
                || !metadata_valid
                || p.node.is_empty()
                || !self.inputs.contains_key(&p.file)
                || !ids.insert((&p.mod_id, &p.injection_id))
            {
                return Err(Diagnostic::new(
                    "E_PLAN_SITE",
                    "invalid call fields, AST path, input file or duplicate injection in plan",
                )
                .file(&p.file));
            }
        }
        for b in &self.bridges {
            source::relative(Path::new(&b.file))?;
            if b.node.is_empty()
                || !self.inputs.contains_key(&b.file)
                || !ids.insert((&b.mod_id, &b.bridge_id))
            {
                return Err(Diagnostic::new(
                    "E_PLAN_SITE",
                    "invalid bridge location or duplicate id",
                ));
            }
        }
        let mut names = std::collections::BTreeSet::new();
        let mut initializers = std::collections::BTreeSet::new();
        for s in &self.states {
            source::relative(Path::new(&s.file))?;
            if s.node.is_empty()
                || !self.inputs.contains_key(&s.file)
                || !ids.insert((&s.mod_id, &s.state_id))
                || !s.name.starts_with("__horizon_mixins_")
                || s.constructors.is_empty()
            {
                return Err(Diagnostic::new(
                    "E_PLAN_SITE",
                    "invalid state location, name or duplicate id",
                ));
            }
            if !names.insert((s.target.clone(), s.name.clone()))
                || !names.insert((s.target.clone(), format!("{}_mut", s.name)))
                || !initializers.insert((
                    &s.file,
                    &s.node[..s.node.len() - 1],
                    format!("{}_init", s.name),
                ))
            {
                return Err(Diagnostic::new(
                    "E_STATE_CONFLICT",
                    "normalized state names collide",
                ));
            }
            s.field()?;
            s.items()?;
            s.initializer()?;
            let mut sites = std::collections::BTreeSet::new();
            for c in &s.constructors {
                source::relative(Path::new(&c.file))?;
                if c.node.is_empty()
                    || !self.inputs.contains_key(&c.file)
                    || !sites.insert((&c.file, &c.node, c.expression))
                {
                    return Err(Diagnostic::new(
                        "E_PLAN_SITE",
                        "invalid state constructor site",
                    ));
                }
                if self
                    .injections
                    .iter()
                    .any(|p| p.file == c.file && p.node == c.node && p.kind == Kind::Overwrite)
                {
                    return Err(Diagnostic::new(
                        "E_STATE_CONFLICT",
                        "OVERWRITE removes a planned state constructor",
                    ));
                }
            }
            if self.bridges.iter().any(|b| {
                b.owner == s.target && [s.name.clone(), format!("{}_mut", s.name)].contains(&b.name)
            }) {
                return Err(Diagnostic::new(
                    "E_STATE_CONFLICT",
                    "state accessor conflicts with bridge",
                ));
            }
        }
        Ok(())
    }
}

pub fn rustc(root: &Path) -> Result<String> {
    let out = Command::new("rustc")
        .current_dir(root)
        .arg("--version")
        .output()
        .map_err(io)?;
    if !out.status.success() {
        return Err(Diagnostic::new(
            "E_TOOLCHAIN",
            String::from_utf8_lossy(&out.stderr),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().into())
}
pub fn build_plan(workspace: &Path, mods: &Path) -> Result<Plan> {
    build_staged(workspace, mods).map(|(plan, _)| plan)
}
/// The plan plus the bundle crate and data files it stages (see [`crate::native::stage`]).
pub fn build_staged(workspace: &Path, mods: &Path) -> Result<(Plan, crate::native::Extras)> {
    let root = workspace.canonicalize().map_err(io)?;
    let mut mods = manifest::load(mods)?;
    let inputs: BTreeMap<String, String> = source::files(&root)?
        .into_iter()
        .map(|(name, bytes)| (name, source::fingerprint(bytes)))
        .collect();
    crate::native::resolve_hooks(&root, &mut mods)?;
    let index = Index::load(&root)?;
    let native = crate::native::plan(&root, &index.packages, &index.bins, &inputs, &mods)?;
    let mut injections = Vec::new();
    let mut planned_bridges = Vec::new();
    let mut planned_states = Vec::new();
    let mut planned_modules: Vec<PlannedModule> = Vec::new();
    for bundle in mods {
        if let Some(revision) = &bundle.manifest.compatibility.horizon_revision {
            let actual = if root.join(".horizon-revision").is_file() {
                std::fs::read_to_string(root.join(".horizon-revision")).map_err(io)?
            } else {
                let output = Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(["rev-parse", "HEAD"])
                    .output()
                    .map_err(io)?;
                if !output.status.success() {
                    return Err(Diagnostic::new(
                        "E_REVISION",
                        "cannot establish a pinned Horizon revision",
                    )
                    .file(&bundle.file));
                }
                String::from_utf8_lossy(&output.stdout).into_owned()
            };
            if revision.len() != 40
                || !revision.bytes().all(|b| b.is_ascii_hexdigit())
                || actual.trim() != revision
            {
                return Err(Diagnostic::new(
                    "E_REVISION",
                    format!(
                        "requires exact Horizon revision {revision}, found {}",
                        actual.trim()
                    ),
                )
                .file(&bundle.file));
            }
        }
        for state in &bundle.manifest.state {
            planned_states.push(
                crate::state::plan(
                    &index,
                    &bundle.manifest.info.id,
                    &bundle.manifest.info.version,
                    bundle.manifest.info.priority,
                    state,
                )
                .map_err(|e| e.file(&bundle.file))?,
            );
        }
        for (module, payload) in bundle.manifest.module.iter().zip(&bundle.modules) {
            let tree = index.files.get(&module.file).ok_or_else(|| {
                Diagnostic::new(
                    "E_MODULE",
                    "module destination must be an indexed Rust source file",
                )
                .file(&module.file)
            })?;
            if index::cfg(&tree.attrs) {
                return Err(Diagnostic::new(
                    "E_UNSUPPORTED_CFG",
                    "support module destination has unresolved cfg",
                ));
            }
            if index::macro_attributes(&tree.attrs) {
                return Err(Diagnostic::new(
                    "E_UNSUPPORTED_MACRO",
                    "support module destination has an attribute macro",
                ));
            }
            if index
                .source_owners
                .get(&module.file)
                .is_none_or(|owners| owners.len() != 1)
            {
                return Err(Diagnostic::new(
                    "E_TARGET_AMBIGUOUS",
                    "support module destination is reused under multiple source identities",
                ));
            }
            crate::modules::validate_destination(tree, &module.name)?;
            if planned_modules
                .iter()
                .any(|m| m.file == module.file && m.name == module.name)
            {
                return Err(
                    Diagnostic::new("E_MODULE_CONFLICT", "competing support modules")
                        .file(&module.file),
                );
            }
            crate::modules::validate_payload(payload)?;
            planned_modules.push(PlannedModule {
                mod_id: bundle.manifest.info.id.clone(),
                mod_version: bundle.manifest.info.version.clone(),
                module_id: module.id.clone(),
                file: module.file.clone(),
                name: module.name.clone(),
                visibility: module.visibility,
                fingerprint: source::fingerprint(tokens(tree)),
                source: payload.clone(),
            });
        }
        for bridge in &bundle.manifest.bridge {
            let planned = bridges::plan(
                &index,
                &bundle.manifest.info.id,
                &bundle.manifest.info.version,
                bridge,
            )
            .map_err(|mut e| {
                e.mod_id = Some(bundle.manifest.info.id.clone().into());
                e.injection_id = Some(bridge.id.clone().into());
                e.target = Some(bridge.target.clone().into());
                e.file(&bundle.file)
            })?;
            if planned_bridges.iter().any(|p: &bridges::PlannedBridge| {
                p.owner == planned.owner && p.name == planned.name
            }) {
                return Err(Diagnostic::new(
                    "E_BRIDGE_CONFLICT",
                    format!(
                        "duplicate generated method {} on {}",
                        planned.name, planned.owner
                    ),
                )
                .file(&bundle.file));
            }
            planned_bridges.push(planned);
        }
        for (injection, helper) in bundle.manifest.injection.iter().zip(&bundle.helpers) {
            let context = |e: Diagnostic| {
                let e = e.injection(&bundle.manifest.info.id, injection);
                if e.file.is_none() {
                    e.file(&bundle.file)
                } else {
                    e
                }
            };
            let f = index.resolve(&injection.target).map_err(context)?;
            let hidden = validate_target(&index, f, injection.kind).map_err(context)?;
            crate::standard_macros::check_sites(&hidden, injection)
                .map_err(|e| context(e.file(&f.file)))?;
            if injection.kind == Kind::Return
                && !sites::is_unit(&helpers::return_type(&f.signature))
                && !matches!(f.block.stmts.last(), Some(syn::Stmt::Expr(_, None)))
                && !sites::block_always_exits(&f.block)
            {
                return Err(context(Diagnostic::new("E_UNSUPPORTED_SITE", "RETURN requires a value tail or syntactically proven explicit exits for a non-unit/aliased result; write an explicit tail expression").file(&f.file)));
            }
            let function_fingerprint =
                source::fingerprint(format!("{}{}", tokens(&f.signature), tokens(&f.block)));
            if injection
                .fingerprint
                .as_ref()
                .is_some_and(|expected| expected != &function_fingerprint)
            {
                return Err(context(Diagnostic::new(
                    "E_FINGERPRINT",
                    format!(
                        "original function changed; expected {}, got {function_fingerprint}",
                        injection.fingerprint.as_ref().unwrap()
                    ),
                )));
            }
            let mut context_function = f.clone();
            if injection.kind == Kind::Head {
                context_function.block = syn::parse_quote!({});
            }
            let catalog = Sites::in_block(&context_function.block);
            let scopes = Scopes::analyze(&context_function);
            if catalog.expressions != scopes.contexts.len() {
                return Err(context(Diagnostic::new(
                    "E_SCOPE_INDEX",
                    "scope/expression coordinates disagree",
                )));
            }
            let slice = injection
                .slice
                .as_ref()
                .map(|s| slices::resolve(s, &catalog, &scopes))
                .transpose()
                .map_err(context)?;
            let inside = |expression: usize| {
                slice
                    .as_ref()
                    .is_none_or(|s| s.start <= expression && expression < s.end)
            };
            let variable_selected = if let Some(variable) = &injection.variable {
                let matches: Vec<_> = scopes
                    .variable_sites(variable)
                    .map_err(context)?
                    .into_iter()
                    .filter(|s| inside(s.expression))
                    .collect();
                if matches.len() != injection.require.unwrap() {
                    return Err(context(Diagnostic::new(
                        "E_MATCH_COUNT",
                        format!(
                            "variable {} requires {}, found {} accesses",
                            variable.name,
                            injection.require.unwrap(),
                            matches.len()
                        ),
                    )));
                }
                let site = *matches.get(injection.ordinal.unwrap()).ok_or_else(|| {
                    context(Diagnostic::new(
                        "E_ORDINAL",
                        "variable access ordinal is out of range",
                    ))
                })?;
                if !site.supported || scopes.contexts[site.expression].constant {
                    return Err(context(Diagnostic::new(
                        "E_UNSUPPORTED_LOCAL",
                        "selected variable access is a place/compound write or unsupported context",
                    )));
                }
                if let Some((code, why)) =
                    modified_value_error(&catalog.values[site.expression].expr)
                {
                    return Err(context(Diagnostic::new(code, why).file(&f.file)));
                }
                Some(site)
            } else {
                None
            };
            let selected = if injection.kind.selects_site()
                && injection.kind != Kind::ModifyVariable
            {
                let selector = injection.selector.as_ref().unwrap();
                let matches: Vec<_> = if injection.kind == Kind::ModifyConstant {
                    let key = sites::literal_selector(selector).map_err(context)?;
                    catalog
                        .constants
                        .iter()
                        .filter(|s| {
                            inside(s.expression)
                                && sites::constant_key(&s.expr).as_ref() == Some(&key)
                        })
                        .collect()
                } else if injection.kind == Kind::ModifyExpressionValue {
                    let expression: Expr = syn::parse_str(selector)
                        .map_err(|e| context(Diagnostic::new("E_SELECTOR", e.to_string())))?;
                    let key = tokens(&expression);
                    catalog
                        .values
                        .iter()
                        .filter(|s| inside(s.expression) && tokens(&s.expr) == key)
                        .collect()
                } else {
                    let expression = sites::parse_call_selector(selector).map_err(context)?;
                    catalog
                        .calls
                        .iter()
                        .filter(|s| {
                            inside(s.expression) && sites::call_matches(&s.expr, &expression)
                        })
                        .collect()
                };
                if matches.len() != injection.require.unwrap() {
                    return Err(context(Diagnostic::new("E_MATCH_COUNT", format!("selector '{selector}' requires {} syntactic matches, found {} in pristine {}", injection.require.unwrap(), matches.len(), f.id)).file(&f.file)));
                }
                let ordinal = injection.ordinal.unwrap();
                let site = matches.get(ordinal).ok_or_else(|| {
                    context(
                        Diagnostic::new(
                            "E_ORDINAL",
                            format!(
                                "ordinal {ordinal} is outside 0..{} for '{selector}'",
                                matches.len()
                            ),
                        )
                        .file(&f.file),
                    )
                })?;
                if site.const_context {
                    return Err(context(Diagnostic::new("E_UNSUPPORTED_SITE", "injection cannot introduce runtime helpers into a compile-time array length or const argument").file(&f.file)));
                }
                if (injection.kind == Kind::BeforeCall
                    || (injection.kind == Kind::AfterCall && injection.cancellable))
                    && !site.whole_expression
                {
                    return Err(context(Diagnostic::new("E_UNSUPPORTED_SITE", "BEFORE_CALL requires a complete statement, plain let initializer, or block/function tail; nesting can change temporary drop timing").file(&f.file)));
                }
                if injection.kind == Kind::AfterAssign
                    && !scopes.after_assign.contains_key(&site.expression)
                {
                    return Err(context(Diagnostic::new(
                        "E_UNSUPPORTED_SITE",
                        "AFTER_ASSIGN requires a complete call initializer of a plain let, or the complete RHS of a simple local/parameter assignment statement",
                    )));
                }
                if injection.kind == Kind::ModifyArg {
                    let arguments = match &site.expr {
                        Expr::Call(c) => c.args.len(),
                        Expr::MethodCall(c) => c.args.len(),
                        _ => unreachable!(),
                    };
                    if injection.argument.unwrap() >= arguments {
                        return Err(context(Diagnostic::new("E_ARGUMENT", format!("argument {} is outside 0..{arguments}; receiver is not an argument", injection.argument.unwrap())).file(&f.file)));
                    }
                }
                Some(*site)
            } else {
                None
            };
            let contract = helpers::contract(
                f,
                injection.kind,
                injection.cancellable,
                helper,
                &injection.bindings,
                selected.map(|s| &s.expr),
                injection.result.as_deref(),
            )
            .map_err(context)?;
            validate_helper(
                f,
                injection.kind,
                injection.cancellable,
                helper,
                &injection.bindings,
                &contract,
            )
            .map_err(context)?;
            let expression_site = variable_selected
                .map(|s| s.expression)
                .or_else(|| selected.map(|s| s.expression));
            let contexts: Vec<_> = if let Some(site) = expression_site {
                vec![if injection.kind == Kind::AfterAssign {
                    &scopes.after_assign[&site]
                } else {
                    &scopes.contexts[site]
                }]
            } else if injection.kind == Kind::Return {
                let mut points: Vec<_> = scopes
                    .returns
                    .iter()
                    .map(|i| &scopes.contexts[*i])
                    .collect();
                if !sites::block_always_exits(&f.block) {
                    points.push(scopes.end.as_ref().unwrap());
                }
                points
            } else {
                vec![]
            };
            let captures = scopes::resolve_bindings(
                f,
                &scopes,
                &injection.bindings,
                &contexts,
                injection.kind,
            )
            .map_err(context)?;
            if injection.kind == Kind::ModifyExpressionValue {
                let site = selected.unwrap();
                sites::validate_value_site(
                    &site.expr,
                    scopes.contexts[site.expression].place,
                    contract.value.as_deref().unwrap(),
                )
                .map_err(context)?;
            }
            if let Some(variable) = &injection.variable {
                let local = &scopes.locals[variable_selected.unwrap().local];
                let value_type = contract.value.as_ref().unwrap();
                if let Some(ty) = &local.ty
                    && helpers::normalize_type(&syn::parse_str::<syn::Type>(ty).unwrap())
                        != helpers::normalize_type(
                            &syn::parse_str::<syn::Type>(value_type).unwrap(),
                        )
                {
                    return Err(context(Diagnostic::new(
                        "E_VARIABLE_TYPE",
                        "variable modifier value type disagrees with its declaration",
                    )));
                }
                if variable.at == manifest::Access::Load
                    && ![
                        "bool", "char", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16",
                        "i32", "i64", "i128", "isize", "f32", "f64",
                    ]
                    .contains(&value_type.as_str())
                {
                    return Err(context(Diagnostic::new(
                        "E_UNSUPPORTED_LOCAL",
                        "LOAD modifiers currently require a primitive scalar value type",
                    )));
                }
            }
            if (contract.bundle.is_some() || contract.operation.is_some())
                && let Some(Expr::MethodCall(call)) = selected.map(|s| &s.expr)
                && let Expr::Path(path) = &*call.receiver
                && path.path.segments.len() == 1
            {
                let name = path.path.segments[0].ident.to_string();
                if scopes.contexts[expression_site.unwrap()]
                    .visible
                    .get(&name)
                    .is_none_or(|id| !scopes.locals[*id].parameter)
                {
                    return Err(context(Diagnostic::new(
                        "E_INVALID_BINDING",
                        "receiver parameter is shadowed at this site",
                    )));
                }
            }
            if let Some(bundle) = &contract.bundle {
                let mut inputs = bundle.inputs.clone();
                if let Some(receiver) = &bundle.receiver {
                    inputs.insert(0, receiver.clone());
                }
                validate_operation_site(
                    f,
                    &selected.unwrap().expr,
                    &helpers::Operation {
                        inputs,
                        output: bundle.output.clone(),
                        method: bundle.receiver.is_some(),
                    },
                )
                .map_err(context)?;
            }
            if let Some(operation) = &contract.operation {
                validate_operation_site(f, &selected.unwrap().expr, operation).map_err(context)?;
            }
            let call_site = selected
                .filter(|_| injection.kind.selects_call())
                .map(|s| s.index);
            let constant_site = selected
                .filter(|_| injection.kind == Kind::ModifyConstant)
                .map(|s| s.index);
            let site_fingerprint = variable_selected
                .map(|s| s.fingerprint.clone())
                .or_else(|| selected.map(|s| s.fingerprint.clone()));
            injections.push(PlannedInjection {
                mod_id: bundle.manifest.info.id.clone(),
                mod_version: bundle.manifest.info.version.clone(),
                injection_id: injection.id.clone(),
                priority: injection.priority.unwrap_or(bundle.manifest.info.priority),
                target: f.id.clone(),
                hook: injection.hook.clone(),
                hook_version: injection.hook_version,
                file: f.file.clone(),
                node: f.node.clone(),
                function_fingerprint,
                kind: injection.kind,
                cancellable: injection.cancellable,
                argument: injection.argument,
                constant_site,
                contract,
                call_site,
                site_fingerprint,
                selector: injection.selector.clone(),
                ordinal: injection.ordinal,
                require: injection.require,
                helper_source: helper.clone(),
                bindings: injection.bindings.clone(),
                captures,
                expression_site,
                variable_site: variable_selected.map(|s| s.expression),
                variable: variable_selected.map(|s| VariableIdentity {
                    local: scopes.locals[s.local].clone(),
                    at: injection.variable.as_ref().unwrap().at,
                }),
                slice,
            });
        }
    }
    planned_states.sort_by(|a, b| {
        (&a.file, &a.node)
            .cmp(&(&b.file, &b.node))
            .then(b.priority.cmp(&a.priority))
            .then(a.mod_id.cmp(&b.mod_id))
            .then(a.state_id.cmp(&b.state_id))
    });
    planned_modules.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.name.cmp(&b.name))
            .then(a.mod_id.cmp(&b.mod_id))
    });
    planned_bridges.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.node.cmp(&b.node))
            .then(a.mod_id.cmp(&b.mod_id))
            .then(a.bridge_id.cmp(&b.bridge_id))
    });
    for p in &injections {
        if p.kind != Kind::Return {
            continue;
        }
        let f = index.resolve(&p.target)?;
        let scopes = Scopes::analyze(f);
        for cancel in injections
            .iter()
            .filter(|c| c.file == p.file && c.node == p.node && c.cancellable)
        {
            let mut entry = f.clone();
            entry.block = syn::parse_quote!({});
            let entry_scopes = Scopes::analyze(&entry);
            let context = if cancel.kind == Kind::Head {
                entry_scopes.end.as_ref().unwrap()
            } else if cancel.kind == Kind::AfterAssign {
                &scopes.after_assign[&cancel.expression_site.unwrap()]
            } else {
                &scopes.contexts[cancel.expression_site.unwrap()]
            };
            scopes::resolve_bindings(f, &scopes, &p.bindings, &[context], Kind::Return)
                .map_err(|e| e.file(&p.file))?;
        }
    }
    injections.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.node.cmp(&b.node))
            .then(a.call_site.cmp(&b.call_site))
            .then(a.constant_site.cmp(&b.constant_site))
            .then(b.priority.cmp(&a.priority))
            .then(a.mod_id.cmp(&b.mod_id))
            .then(a.injection_id.cmp(&b.injection_id))
    });
    let mut groups: BTreeMap<(&str, &[usize]), Vec<&PlannedInjection>> = BTreeMap::new();
    for p in &injections {
        groups.entry((&p.file, &p.node)).or_default().push(p);
    }
    for entries in groups.values() {
        if entries.len() > 1 && entries.iter().any(|p| p.kind == Kind::Overwrite) {
            let p = entries[0];
            return Err(Diagnostic {
                mod_id: Some(p.mod_id.clone().into()),
                injection_id: Some(p.injection_id.clone().into()),
                target: Some(p.target.clone().into()),
                ..Diagnostic::new(
                    "E_OVERWRITE_CONFLICT",
                    format!(
                        "OVERWRITE must be exclusive on {}; competing injections: {}",
                        p.target,
                        entries
                            .iter()
                            .map(|p| format!("{}/{}", p.mod_id, p.injection_id))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
                .file(&p.file)
            });
        }
    }
    for entries in groups.values() {
        validate_composition(entries)?;
    }
    // Ensure metadata did not change inputs and catch concurrent source edits.
    let after: BTreeMap<_, _> = source::files(&root)?
        .into_iter()
        .map(|(name, bytes)| (name, source::fingerprint(bytes)))
        .collect();
    if inputs != after {
        return Err(Diagnostic::new(
            "E_STALE_SOURCE",
            "workspace changed during planning; retry from pristine source",
        ));
    }
    let plan = Plan {
        schema: SCHEMA,
        transformer: env!("CARGO_PKG_VERSION").into(),
        rustc: rustc(&root)?,
        inputs,
        injections,
        bridges: planned_bridges,
        modules: planned_modules,
        states: planned_states,
        crates: native.crates,
        data: native.data,
        post_build: native.post_build,
    };
    plan.validate_structure()?;
    Ok((plan, native.extras))
}
pub(crate) fn validate_target(index: &Index, f: &Function, kind: Kind) -> Result<Vec<Expr>> {
    let s = &f.signature;
    if s.ident.to_string().starts_with("__horizon_mixins_") {
        return Err(Diagnostic::new(
            "E_RESERVED_NAME",
            "generated bridge names cannot be injection targets",
        )
        .file(&f.file));
    }
    if s.asyncness.is_some()
        || s.constness.is_some()
        || s.unsafety.is_some()
        || s.abi.is_some()
        || s.variadic.is_some()
        || !s.generics.params.is_empty()
        || s.generics.where_clause.is_some()
        || f.generic_impl
    {
        return Err(Diagnostic::new(
            "E_UNSUPPORTED_SIGNATURE",
            "MVP requires a synchronous, safe, non-const, non-generic Rust function",
        )
        .file(&f.file));
    }
    let mut safety = Safety {
        error: None,
        boundary: kind == Kind::Head,
        // RETURN moves only exit values (`return` operands and the tail) into a helper call
        // argument and leaves the rest of the body as it was. References to calls and closures
        // are checked in those values only ([`exit_value_error`]). MODIFY_VARIABLE likewise
        // replaces only the value it selects ([`modified_value_error`]).
        rewrites_body: !matches!(kind, Kind::Return | Kind::ModifyVariable),
        // A `?` is an exit RETURN would miss; MODIFY_VARIABLE leaves one outside its value alone.
        try_outside_value: kind == Kind::ModifyVariable,
        body_macros: false,
        in_body: false,
        macro_depth: 0,
        policy: crate::standard_macros::Policy::for_function(index, f),
        macro_error: None,
        hidden: Vec::new(),
    };
    // Signature macros still require semantic resolution. Only preserved body
    // expression/statement invocations may remain opaque.
    safety.visit_signature(s);
    // HEAD cancellation returns before the preserved body; it does not rewrite
    // any exit or expression inside a macro, just like ordinary HEAD.
    safety.body_macros = safety.boundary;
    safety.in_body = true;
    safety.visit_block(&f.block);
    if safety.error.is_none() && kind == Kind::Return {
        safety.error = exit_value_error(&f.block);
    }
    if let Some((code, why)) = safety.error {
        return Err(Diagnostic::new(code, why).file(&f.file));
    }
    if let Some(error) = safety.macro_error {
        return Err(error.file(&f.file));
    }
    Ok(safety.hidden)
}

const REFERENCED_CALL: (&str, &str) = (
    "E_UNSUPPORTED_CONTROL_FLOW",
    "calls beneath a reference may change temporary lifetimes",
);
const CLOSURES_ETC: (&str, &str) = (
    "E_UNSUPPORTED_CONTROL_FLOW",
    "closures, async, try/?, unsafe and const blocks are unsupported injection contexts",
);
/// What RETURN refuses in the values it moves into a helper argument: each `return` operand
/// and the body's tail. A call beneath a reference there could change temporary lifetimes. A
/// plain closure there moves into the argument with the value, and what it holds is not part of
/// that value: a `return` inside it leaves the closure, and RETURN does not rewrite it. The
/// whole-body check still refuses async closures and `?` or unsafe inside any closure.
fn exit_value_error(block: &syn::Block) -> Option<(&'static str, &'static str)> {
    struct Find {
        exits: usize,
        error: Option<(&'static str, &'static str)>,
    }
    impl<'ast> Visit<'ast> for Find {
        fn visit_expr(&mut self, e: &'ast Expr) {
            match e {
                Expr::Closure(_) => return,
                Expr::Return(r) => {
                    self.exits += 1;
                    visit::visit_expr_return(self, r);
                    self.exits -= 1;
                    return;
                }
                Expr::Reference(r) if self.exits > 0 && contains_call(&r.expr) => {
                    self.error = Some(REFERENCED_CALL)
                }
                _ => {}
            }
            visit::visit_expr(self, e);
        }
    }
    let mut find = Find {
        exits: 0,
        error: None,
    };
    for (i, stmt) in block.stmts.iter().enumerate() {
        match stmt {
            syn::Stmt::Expr(e, None) if i + 1 == block.stmts.len() => {
                find.exits += 1;
                find.visit_expr(e);
                find.exits -= 1;
            }
            _ => find.visit_stmt(stmt),
        }
    }
    find.error
}
/// What MODIFY_VARIABLE refuses in the one value it replaces (a read, or the value a `let` or an
/// assignment stores): a closure, a `?`, or a call beneath a reference, whose meaning wrapping
/// the value could change. The rest of the body is left as it was.
fn modified_value_error(value: &Expr) -> Option<(&'static str, &'static str)> {
    struct Find(Option<(&'static str, &'static str)>);
    impl<'ast> Visit<'ast> for Find {
        fn visit_expr(&mut self, e: &'ast Expr) {
            match e {
                Expr::Closure(_) | Expr::Try(_) => self.0 = Some(CLOSURES_ETC),
                Expr::Reference(r) if contains_call(&r.expr) => self.0 = Some(REFERENCED_CALL),
                _ => {}
            }
            visit::visit_expr(self, e);
        }
    }
    let mut find = Find(None);
    find.visit_expr(value);
    find.0
}
struct Safety {
    error: Option<(&'static str, &'static str)>,
    boundary: bool,
    rewrites_body: bool,
    try_outside_value: bool,
    body_macros: bool,
    in_body: bool,
    macro_depth: usize,
    policy: crate::standard_macros::Policy,
    macro_error: Option<Diagnostic>,
    hidden: Vec<Expr>,
}
impl<'ast> Visit<'ast> for Safety {
    fn visit_pat(&mut self, pat: &'ast syn::Pat) {
        // syn represents pattern macros with ExprMacro too; they are not
        // covered by the preserved expression/statement exception.
        if let syn::Pat::Macro(mac) = pat {
            self.error = Some(("E_UNSUPPORTED_MACRO", "pattern macros are unsupported"));
            for attr in &mac.attrs {
                self.visit_attribute(attr);
            }
        } else {
            visit::visit_pat(self, pat);
        }
    }
    fn visit_pat_ident(&mut self, pat: &'ast syn::PatIdent) {
        if pat.ident.to_string().starts_with("__horizon_mixins_") {
            self.error = Some((
                "E_RESERVED_NAME",
                "binding names beginning __horizon_mixins_ are reserved for generated code",
            ));
        }
        visit::visit_pat_ident(self, pat);
    }
    fn visit_expr(&mut self, expr: &'ast Expr) {
        if self.boundary {
            visit::visit_expr(self, expr);
            return;
        }
        if self.macro_depth > 0
            && matches!(
                expr,
                Expr::Return(_)
                    | Expr::Break(_)
                    | Expr::Continue(_)
                    | Expr::Try(_)
                    | Expr::Closure(_)
                    | Expr::Async(_)
                    | Expr::Await(_)
                    | Expr::TryBlock(_)
                    | Expr::Yield(_)
                    | Expr::Unsafe(_)
                    | Expr::Const(_)
                    | Expr::Verbatim(_)
            )
        {
            self.error = Some((
                "E_UNSUPPORTED_MACRO",
                "macro inputs contain hidden control flow; move it outside the macro",
            ));
            return;
        }
        match expr {
            // RETURN never rewrites inside a closure; what is inside is still checked.
            Expr::Closure(c) if !self.rewrites_body && c.asyncness.is_none() => {}
            Expr::Try(_) if self.try_outside_value => {}
            Expr::Closure(_)
            | Expr::Async(_)
            | Expr::Await(_)
            | Expr::Try(_)
            | Expr::TryBlock(_)
            | Expr::Yield(_)
            | Expr::Unsafe(_)
            | Expr::Const(_) => self.error = Some(CLOSURES_ETC),
            // Calls under a reference can depend on temporary lifetime extension.
            Expr::Reference(r)
                if self.rewrites_body && self.macro_depth == 0 && contains_call(&r.expr) =>
            {
                self.error = Some(REFERENCED_CALL)
            }
            _ => {}
        }
        visit::visit_expr(self, expr);
    }
    fn visit_expr_macro(&mut self, expr: &'ast syn::ExprMacro) {
        if self.body_macros {
            for attr in &expr.attrs {
                self.visit_attribute(attr);
            }
        } else {
            visit::visit_expr_macro(self, expr);
        }
    }
    fn visit_stmt_macro(&mut self, stmt: &'ast syn::StmtMacro) {
        if self.body_macros {
            for attr in &stmt.attrs {
                self.visit_attribute(attr);
            }
        } else {
            visit::visit_stmt_macro(self, stmt);
        }
    }
    fn visit_type_macro(&mut self, _: &'ast syn::TypeMacro) {
        self.error = Some(("E_UNSUPPORTED_MACRO", "type macros are unsupported"));
    }
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if !self.in_body {
            self.error = Some(("E_UNSUPPORTED_MACRO", "signature macros are unsupported"));
            return;
        }
        match self.policy.parse(mac) {
            Ok(args) => {
                self.macro_depth += 1;
                if let Some(pat) = &args.pattern {
                    self.visit_pat(pat);
                }
                for arg in &args.expressions {
                    self.visit_expr(arg);
                }
                self.macro_depth -= 1;
                self.hidden.extend(args.expressions);
            }
            Err(error) => self.macro_error = Some(error),
        }
    }
    fn visit_item(&mut self, item: &'ast Item) {
        if self.boundary {
            visit::visit_item(self, item);
            return;
        }
        self.error = Some((
            "E_UNSUPPORTED_CONTROL_FLOW",
            "nested items in target functions are unsupported",
        ));
    }
    fn visit_attribute(&mut self, attr: &'ast syn::Attribute) {
        if index::macro_attributes(std::slice::from_ref(attr)) {
            self.error = Some((
                "E_UNSUPPORTED_MACRO",
                "target contains an unexpanded attribute macro",
            ));
        }
        if attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr") {
            self.error = Some((
                "E_UNSUPPORTED_CFG",
                "target body contains unresolved cfg/cfg_attr",
            ));
        }
    }
}
fn contains_call(expr: &Expr) -> bool {
    struct Find(bool);
    impl<'ast> Visit<'ast> for Find {
        fn visit_expr(&mut self, e: &'ast Expr) {
            if is_call(e) {
                self.0 = true;
            }
            visit::visit_expr(self, e);
        }
    }
    let mut f = Find(false);
    f.visit_expr(expr);
    f.0
}
pub use crate::helpers::{parse_helper, validate_helper};
pub use crate::sites::{call_key, is_call};

fn validate_operation_site(
    f: &Function,
    expr: &Expr,
    operation: &helpers::Operation,
) -> Result<()> {
    match expr {
        Expr::Call(call) => {
            let Expr::Path(path) = &*call.func else {
                return Err(Diagnostic::new(
                    "E_UNSUPPORTED_SITE",
                    "wrapping requires a module-level callable path",
                ));
            };
            if path.path.segments.len() == 1 {
                let name = path.path.segments[0].ident.to_string();
                struct Locals(std::collections::BTreeSet<String>);
                impl<'ast> Visit<'ast> for Locals {
                    fn visit_pat_ident(&mut self, pat: &'ast syn::PatIdent) {
                        self.0.insert(pat.ident.to_string());
                        visit::visit_pat_ident(self, pat);
                    }
                }
                let mut names = Locals(index::parameters(&f.signature)?);
                names.visit_block(&f.block);
                if names.0.contains(&name) {
                    return Err(Diagnostic::new(
                        "E_UNSUPPORTED_SITE",
                        "wrapping a local callable value is unsupported; select a module-level function",
                    ));
                }
            }
        }
        Expr::MethodCall(call) => {
            let Expr::Path(receiver) = &*call.receiver else {
                return Err(Diagnostic::new(
                    "E_UNSUPPORTED_SITE",
                    "wrapped method receiver must be a target reference parameter",
                ));
            };
            if receiver.path.segments.len() != 1 {
                return Err(Diagnostic::new(
                    "E_UNSUPPORTED_SITE",
                    "method receiver must be a single parameter name",
                ));
            }
            let name = receiver.path.segments[0].ident.to_string();
            let ty = f.signature.inputs.iter().find_map(|input| match input {
                syn::FnArg::Typed(arg) if matches!(&*arg.pat, syn::Pat::Ident(p) if p.ident == name) => Some(*arg.ty.clone()),
                syn::FnArg::Receiver(r) if name == "self" && r.reference.is_some() => f.self_type.as_ref().map(|ty| {
                    if r.mutability.is_some() { syn::parse_quote!(&mut #ty) } else { syn::parse_quote!(&#ty) }
                }),
                _ => None,
            });
            let declared: syn::Type = syn::parse_str(&operation.inputs[0])
                .map_err(|e| Diagnostic::new("E_OPERATION_SIGNATURE", e.to_string()))?;
            let Some(syn::Type::Reference(reference)) = ty else {
                return Err(Diagnostic::new(
                    "E_UNSUPPORTED_SITE",
                    "wrapped method receiver must be a target parameter with an explicit reference type",
                ));
            };
            if helpers::normalize_type(&syn::Type::Reference(reference.clone()))
                != helpers::normalize_type(&declared)
            {
                return Err(Diagnostic::new(
                    "E_OPERATION_SIGNATURE",
                    "receiver binding must match the original parameter reference type",
                ));
            }
            if reference.mutability.is_some() {
                struct Mentions<'a> {
                    name: &'a str,
                    found: bool,
                }
                impl<'ast> Visit<'ast> for Mentions<'_> {
                    fn visit_expr_path(&mut self, p: &'ast syn::ExprPath) {
                        self.found |= p
                            .path
                            .segments
                            .first()
                            .is_some_and(|s| s.ident == self.name);
                        visit::visit_expr_path(self, p);
                    }
                }
                let mut mentions = Mentions {
                    name: &name,
                    found: false,
                };
                for arg in &call.args {
                    mentions.visit_expr(arg);
                }
                if mentions.found {
                    return Err(Diagnostic::new(
                        "E_UNSUPPORTED_SITE",
                        "receiver also occurs in call arguments; two-phase receiver borrowing is unsupported for wrappers",
                    ));
                }
            }
        }
        _ => {
            return Err(Diagnostic::new(
                "E_PLAN_SITE",
                "operation site is not a call",
            ));
        }
    }
    Ok(())
}
fn composition_error(
    p: &PlannedInjection,
    previous: &PlannedInjection,
    code: &str,
    message: &str,
) -> Diagnostic {
    Diagnostic {
        mod_id: Some(p.mod_id.clone().into()),
        injection_id: Some(p.injection_id.clone().into()),
        target: Some(p.target.clone().into()),
        selector: p.selector.as_deref().map(Into::into),
        ..Diagnostic::new(
            code,
            format!(
                "{message}: {}/{} and {}/{} on {}",
                previous.mod_id, previous.injection_id, p.mod_id, p.injection_id, p.target
            ),
        )
        .file(&p.file)
    }
}
fn validate_composition(entries: &[&PlannedInjection]) -> Result<()> {
    let mut operations: BTreeMap<usize, &PlannedInjection> = BTreeMap::new();
    for p in entries {
        if let Some(op) = &p.contract.operation
            && let Some(previous) = operations.insert(p.call_site.unwrap(), p)
            && previous.contract.operation.as_ref() != Some(op)
        {
            return Err(composition_error(
                p,
                previous,
                "E_OPERATION_SIGNATURE",
                "wrappers at one original site must have identical input/output signatures",
            ));
        }
    }
    let mut bundles: BTreeMap<usize, &PlannedInjection> = BTreeMap::new();
    for p in entries {
        if let Some(bundle) = &p.contract.bundle {
            if let Some(previous) = bundles.insert(p.call_site.unwrap(), p)
                && previous.contract.bundle.as_ref() != Some(bundle)
            {
                return Err(composition_error(
                    p,
                    previous,
                    "E_ARGUMENT_SIGNATURE",
                    "tuple modifiers must use identical argument/result signatures",
                ));
            }
            if let Some(wrapper) = operations.get(&p.call_site.unwrap()) {
                let op = wrapper.contract.operation.as_ref().unwrap();
                let expected = &op.inputs[usize::from(op.method)..];
                if expected != bundle.inputs
                    || op.output != bundle.output
                    || op.method != bundle.receiver.is_some()
                {
                    return Err(composition_error(
                        p,
                        wrapper,
                        "E_ARGUMENT_SIGNATURE",
                        "tuple modifier and wrapper signatures disagree",
                    ));
                }
            }
        }
    }
    type ValueSite = (
        Option<usize>,
        Option<usize>,
        Option<usize>,
        Option<usize>,
        Option<usize>,
    );
    let mut values: BTreeMap<ValueSite, &PlannedInjection> = BTreeMap::new();
    for p in entries {
        if matches!(
            p.kind,
            Kind::ModifyArg
                | Kind::ModifyConstant
                | Kind::ModifyVariable
                | Kind::ModifyExpressionValue
        ) {
            let ty = p.contract.value.as_deref().unwrap();
            let key = (
                p.call_site,
                p.constant_site,
                p.variable_site,
                p.argument,
                (p.kind == Kind::ModifyExpressionValue)
                    .then_some(p.expression_site)
                    .flatten(),
            );
            if let Some(previous) = values.insert(key, p)
                && previous.contract.value.as_deref() != Some(ty)
            {
                return Err(composition_error(
                    p,
                    previous,
                    "E_VALUE_SIGNATURE",
                    "modifiers on one original value must use identical input/output types",
                ));
            }
            if p.kind == Kind::ModifyArg
                && let Some(bundle) = bundles.get(&p.call_site.unwrap())
                && bundle.contract.bundle.as_ref().unwrap().inputs[p.argument.unwrap()] != ty
            {
                return Err(composition_error(
                    p,
                    bundle,
                    "E_ARGUMENT_SIGNATURE",
                    "argument modifier disagrees with tuple element type",
                ));
            }
            if p.kind == Kind::ModifyArg
                && let Some(wrapper) = operations.get(&p.call_site.unwrap())
            {
                let op = wrapper.contract.operation.as_ref().unwrap();
                let expected = &op.inputs[p.argument.unwrap() + usize::from(op.method)];
                if expected != ty {
                    return Err(composition_error(
                        p,
                        wrapper,
                        "E_OPERATION_SIGNATURE",
                        "argument modifier type disagrees with the wrapper operation signature",
                    ));
                }
            }
        }
    }
    let mut results: BTreeMap<usize, (&PlannedInjection, String)> = BTreeMap::new();
    for p in entries {
        let ty = if matches!(
            p.kind,
            Kind::ModifyExpressionValue | Kind::ModifyConstant | Kind::ModifyVariable
        ) {
            p.contract.value.clone()
        } else if p.kind == Kind::AfterCall {
            p.contract.result.clone()
        } else if p.kind == Kind::WrapOperation {
            p.contract.operation.as_ref().map(|op| op.output.clone())
        } else if p.kind == Kind::ModifyArgs {
            p.contract.bundle.as_ref().map(|b| b.output.clone())
        } else {
            None
        };
        if let Some(ty) = ty {
            let expression = p.expression_site.unwrap();
            if let Some((previous, expected)) = results.insert(expression, (p, ty.clone()))
                && helpers::normalize_type(&syn::parse_str(&expected).unwrap())
                    != helpers::normalize_type(&syn::parse_str(&ty).unwrap())
            {
                return Err(composition_error(
                    p,
                    previous,
                    "E_VALUE_SIGNATURE",
                    "call result/expression modifier contracts disagree at the same original expression",
                ));
            }
        }
    }
    Ok(())
}

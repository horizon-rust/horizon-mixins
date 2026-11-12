use crate::{Diagnostic, Result, diagnostic::io, source};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(rename = "mod")]
    pub info: Mod,
    #[serde(default)]
    pub compatibility: Compatibility,
    #[serde(default)]
    pub injection: Vec<Injection>,
    #[serde(default)]
    pub bridge: Vec<Bridge>,
    #[serde(default)]
    pub module: Vec<Module>,
    #[serde(default)]
    pub state: Vec<State>,
    #[serde(default, rename = "crate")]
    pub crates: Vec<Crate>,
    #[serde(default)]
    pub post_build: Vec<PostBuild>,
}
/// A bundle's own Cargo package, staged as a workspace member (0.6.0).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crate {
    pub id: String,
    pub path: String,
    pub dependent: Option<String>,
}
impl Crate {
    pub const DEFAULT_DEPENDENT: &str = "horizon-client";
    pub fn dependent(&self) -> &str {
        self.dependent.as_deref().unwrap_or(Self::DEFAULT_DEPENDENT)
    }
}
/// A bin of one of the mod's crates that the launcher runs after the game builds.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostBuild {
    #[serde(rename = "crate")]
    pub crate_id: String,
    pub bin: String,
    #[serde(default)]
    pub args: Vec<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub id: String,
    pub target: String,
    pub ty: String,
    pub initial: String,
    pub priority: Option<i32>,
    pub constructor: Vec<Constructor>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Constructor {
    pub target: String,
    pub selector: String,
    pub require: usize,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Module {
    pub id: String,
    pub file: String,
    pub name: String,
    pub source: String,
    #[serde(default)]
    pub visibility: ModuleVisibility,
}
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleVisibility {
    #[default]
    #[serde(rename = "pub(crate)")]
    Crate,
    #[serde(rename = "pub")]
    Public,
}
impl ModuleVisibility {
    pub fn is_crate(&self) -> bool {
        *self == Self::Crate
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mod {
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub priority: i32,
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compatibility {
    pub horizon_revision: Option<String>,
}
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Kind {
    Head,
    BeforeCall,
    Overwrite,
    Return,
    ModifyArg,
    ModifyConstant,
    WrapOperation,
    ModifyArgs,
    ModifyVariable,
    ModifyExpressionValue,
    AfterCall,
    AfterAssign,
}
impl Kind {
    pub fn selects_call(self) -> bool {
        matches!(
            self,
            Self::BeforeCall
                | Self::AfterCall
                | Self::AfterAssign
                | Self::ModifyArg
                | Self::ModifyArgs
                | Self::WrapOperation
        )
    }
    pub fn selects_site(self) -> bool {
        self.selects_call()
            || matches!(
                self,
                Self::ModifyConstant | Self::ModifyVariable | Self::ModifyExpressionValue
            )
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Injection {
    pub id: String,
    /// Raw target path. Empty when the injection names a catalog `hook` instead; planning
    /// replaces it with the catalog's target before anything else reads it.
    #[serde(default)]
    pub target: String,
    pub hook: Option<String>,
    pub hook_version: Option<u32>,
    pub kind: Kind,
    pub helper: String,
    #[serde(default)]
    pub cancellable: bool,
    pub argument: Option<usize>,
    #[serde(default)]
    pub bindings: Vec<Binding>,
    pub priority: Option<i32>,
    pub selector: Option<String>,
    pub ordinal: Option<usize>,
    pub require: Option<usize>,
    pub fingerprint: Option<String>,
    pub slice: Option<Slice>,
    pub variable: Option<Variable>,
    pub result: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub selector: String,
    pub ordinal: usize,
    pub require: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slice {
    pub from: Option<Anchor>,
    pub to: Option<Anchor>,
}
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Access {
    Store,
    Load,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variable {
    pub name: String,
    pub ordinal: usize,
    pub at: Access,
    #[serde(default)]
    pub parameter: bool,
}
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BridgeKind {
    Get,
    GetMut,
    Set,
    Invoke,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bridge {
    pub id: String,
    pub kind: BridgeKind,
    pub target: String,
    pub member: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub name: String,
    pub ty: String,
    pub value: String,
}
#[derive(Debug)]
pub struct LoadedMod {
    pub manifest: Manifest,
    pub file: String,
    pub helpers: Vec<String>,
    pub modules: Vec<String>,
    /// The bundle directory (`None` for a direct `.toml` mod in the mods root).
    pub dir: Option<PathBuf>,
    /// Each `[[crate]]`'s files, relative to its folder.
    pub crates: Vec<BTreeMap<String, Vec<u8>>>,
    /// The bundle's `data/` files, relative to `data/`.
    pub data: BTreeMap<String, Vec<u8>>,
}
pub(crate) fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
/// Only direct TOML files and one directory level of horizon-mixin.toml bundles.
pub fn load(root: &Path) -> Result<Vec<LoadedMod>> {
    let root = root.canonicalize().map_err(io)?;
    let mut paths = Vec::new();
    if !root.is_dir() {
        return Err(Diagnostic::new("E_INVALID_PATH", "--mods must be a directory").file(&root));
    }
    for entry in fs::read_dir(&root).map_err(io)? {
        let entry = entry.map_err(io)?;
        if entry.file_type().map_err(io)?.is_symlink() {
            return Err(
                Diagnostic::new("E_INVALID_PATH", "mod bundles cannot be symlinks")
                    .file(entry.path()),
            );
        }
        if entry.path().extension().is_some_and(|e| e == "toml") {
            paths.push(entry.path());
        } else if entry.file_type().map_err(io)?.is_dir()
            && entry.path().join("horizon-mixin.toml").exists()
        {
            paths.push(entry.path().join("horizon-mixin.toml"));
        }
    }
    paths.sort();
    if paths.is_empty() {
        return Err(Diagnostic::new("E_MANIFEST", "no TOML mod manifests found").file(&root));
    }
    let mut ids = BTreeSet::new();
    let mut mods = Vec::new();
    for path in paths {
        let path = source::contained(&root, path.strip_prefix(&root).unwrap())?;
        let text = fs::read_to_string(&path).map_err(|e| io(e).file(&path))?;
        let manifest: Manifest = toml::from_str(&text)
            .map_err(|e| Diagnostic::new("E_MANIFEST", e.to_string()).file(&path))?;
        let fail = |code: &str, message: String| Diagnostic::new(code, message).file(&path);
        let fail_path = |message: String| fail("E_INVALID_PATH", message);
        if !valid_id(&manifest.info.id)
            || manifest.info.version.is_empty()
            || (manifest.injection.is_empty()
                && manifest.bridge.is_empty()
                && manifest.module.is_empty()
                && manifest.state.is_empty()
                && manifest.crates.is_empty())
        {
            return Err(fail(
                "E_MANIFEST",
                "mod needs an ASCII id, a version and at least one injection, bridge, module, state or crate"
                    .into(),
            ));
        }
        let bundle = path.parent().unwrap();
        let dir = (path.file_name().is_some_and(|n| n == "horizon-mixin.toml") && bundle != root)
            .then(|| bundle.to_path_buf());
        if !ids.insert(manifest.info.id.clone()) {
            return Err(fail(
                "E_DUPLICATE_ID",
                format!("duplicate mod id '{}'", manifest.info.id),
            ));
        }
        let mut injections = BTreeSet::new();
        let mut helpers = Vec::new();
        let mut modules = Vec::new();
        for state in &manifest.state {
            if !valid_id(&state.id) || !injections.insert(&state.id) {
                return Err(fail(
                    "E_DUPLICATE_ID",
                    format!("invalid or duplicate state id '{}'", state.id),
                ));
            }
        }
        for module in &manifest.module {
            if !valid_id(&module.id) || !injections.insert(&module.id) {
                return Err(fail(
                    "E_DUPLICATE_ID",
                    format!("invalid or duplicate module id '{}'", module.id),
                ));
            }
            forward_slashes("[[module]] file", &module.file).map_err(&fail_path)?;
            forward_slashes("[[module]] source", &module.source).map_err(&fail_path)?;
            source::relative(Path::new(&module.file))?;
            syn::parse_str::<syn::Ident>(&module.name)
                .map_err(|e| fail("E_MODULE", e.to_string()))?;
            let payload = source::contained(path.parent().unwrap(), Path::new(&module.source))?;
            modules.push(fs::read_to_string(&payload).map_err(io)?);
        }
        for bridge in &manifest.bridge {
            if !valid_id(&bridge.id) || !injections.insert(&bridge.id) {
                return Err(fail(
                    "E_DUPLICATE_ID",
                    format!("invalid or duplicate bridge id '{}'", bridge.id),
                ));
            }
            if (bridge.kind == BridgeKind::Invoke) == bridge.member.is_some() {
                return Err(fail("E_BRIDGE_TARGET", "GET/GET_MUT/SET require member; INVOKE uses an exact method target without member".into()));
            }
        }
        for injection in &manifest.injection {
            let context = |e: Diagnostic| e.injection(&manifest.info.id, injection).file(&path);
            if !valid_id(&injection.id) || !injections.insert(&injection.id) {
                return Err(context(fail(
                    "E_DUPLICATE_ID",
                    format!("invalid or duplicate injection id '{}'", injection.id),
                )));
            }
            match (&injection.hook, injection.hook_version) {
                (None, None) if !injection.target.is_empty() => {}
                (Some(hook), Some(_)) if injection.target.is_empty() && !hook.is_empty() => {}
                (None, None) => {
                    return Err(context(fail(
                        "E_MANIFEST",
                        "an injection needs a target or a named hook".into(),
                    )));
                }
                (Some(_), _) if !injection.target.is_empty() => {
                    return Err(context(fail(
                        "E_MANIFEST",
                        "use either target or hook, not both".into(),
                    )));
                }
                (Some(_), None) => {
                    return Err(context(fail(
                        "E_MANIFEST",
                        "hook requires hook_version".into(),
                    )));
                }
                _ => {
                    return Err(context(fail(
                        "E_MANIFEST",
                        "hook_version requires a nonempty hook".into(),
                    )));
                }
            }
            if injection.cancellable
                && !matches!(
                    injection.kind,
                    Kind::Head | Kind::BeforeCall | Kind::AfterCall | Kind::AfterAssign
                )
            {
                return Err(context(fail(
                    "E_MANIFEST",
                    "cancellable requires HEAD, BEFORE_CALL, AFTER_CALL or AFTER_ASSIGN".into(),
                )));
            }
            if (injection.kind == Kind::ModifyArg) != injection.argument.is_some() {
                return Err(context(fail(
                    "E_ARGUMENT",
                    "argument is required only for MODIFY_ARG".into(),
                )));
            }
            if (injection.kind == Kind::ModifyVariable) != injection.variable.is_some() {
                return Err(context(fail(
                    "E_VARIABLE",
                    "variable is required only for MODIFY_VARIABLE".into(),
                )));
            }
            if matches!(injection.kind, Kind::ModifyArgs | Kind::AfterCall)
                != injection.result.is_some()
            {
                return Err(context(fail(
                    "E_ARGUMENT_SIGNATURE",
                    "result is required only for MODIFY_ARGS and AFTER_CALL".into(),
                )));
            }
            if injection.kind.selects_site() {
                if (injection.kind != Kind::ModifyVariable && injection.selector.is_none())
                    || injection.ordinal.is_none()
                    || injection.require.is_none_or(|n| n == 0)
                {
                    return Err(context(fail("E_SELECTOR", "site injections require a selector (or variable), zero-based ordinal and positive require".into())));
                }
                if injection.kind == Kind::ModifyVariable && injection.selector.is_some() {
                    return Err(context(fail(
                        "E_SELECTOR",
                        "MODIFY_VARIABLE uses variable rather than selector".into(),
                    )));
                }
            } else if injection.selector.is_some()
                || injection.ordinal.is_some()
                || injection.require.is_some()
            {
                return Err(context(fail(
                    "E_SELECTOR",
                    "selector/ordinal/require apply only to site injections".into(),
                )));
            }
            if injection.slice.is_some() && !injection.kind.selects_site() {
                return Err(context(fail(
                    "E_SLICE",
                    "slices apply only to site injections".into(),
                )));
            }
            forward_slashes("helper", &injection.helper)
                .map_err(|m| context(Diagnostic::new("E_INVALID_PATH", m)))?;
            let helper_path =
                source::contained(path.parent().unwrap(), Path::new(&injection.helper))
                    .map_err(context)?;
            let helper = fs::read_to_string(&helper_path).map_err(|e| context(io(e)))?;
            helpers.push(helper);
        }
        let mut crates = Vec::new();
        let mut crate_ids = BTreeSet::new();
        for spec in &manifest.crates {
            let context = |e: Diagnostic| {
                let mut e = e.file(&path);
                e.mod_id = Some(manifest.info.id.clone().into());
                e.injection_id = Some(spec.id.clone().into());
                e
            };
            if !valid_id(&spec.id) || !injections.insert(&spec.id) {
                return Err(context(Diagnostic::new(
                    "E_DUPLICATE_ID",
                    format!("invalid or duplicate crate id '{}'", spec.id),
                )));
            }
            crate_ids.insert(spec.id.as_str());
            let Some(dir) = &dir else {
                return Err(context(Diagnostic::new(
                    "E_CRATE",
                    "[[crate]] needs a bundle directory with horizon-mixin.toml",
                )));
            };
            forward_slashes("[[crate]] path", &spec.path)
                .map_err(|m| context(Diagnostic::new("E_INVALID_PATH", m)))?;
            source::relative(Path::new(&spec.path)).map_err(context)?;
            if Path::new(&spec.path).starts_with("data") {
                return Err(context(Diagnostic::new(
                    "E_CRATE",
                    "a crate folder cannot be (or be inside) the bundle's data/ folder",
                )));
            }
            if fs::symlink_metadata(dir.join(&spec.path)).is_ok_and(|m| m.is_symlink()) {
                return Err(context(
                    Diagnostic::new("E_INVALID_PATH", "crate folders cannot be symlinks")
                        .file(dir.join(&spec.path)),
                ));
            }
            let folder = source::contained(dir, Path::new(&spec.path)).map_err(context)?;
            if !folder.is_dir() || !folder.join("Cargo.toml").is_file() {
                return Err(context(Diagnostic::new(
                    "E_CRATE",
                    format!("crate '{}' must be a folder holding Cargo.toml", spec.id),
                )));
            }
            crates.push(source::tree(&folder, true).map_err(context)?);
        }
        for post in &manifest.post_build {
            if !crate_ids.contains(post.crate_id.as_str()) {
                return Err(fail(
                    "E_POST_BUILD",
                    format!(
                        "post_build crate '{}' is not a [[crate]] of mod '{}'",
                        post.crate_id, manifest.info.id
                    ),
                ));
            }
            if !valid_bin(&post.bin) {
                return Err(fail(
                    "E_POST_BUILD",
                    format!("post_build bin '{}' is not a valid bin name", post.bin),
                ));
            }
        }
        let mut data = BTreeMap::new();
        if let Some(dir) = &dir {
            let folder = dir.join("data");
            match fs::symlink_metadata(&folder) {
                Ok(meta) if meta.is_dir() => data = source::tree(&folder, false)?,
                Ok(_) => {
                    return Err(Diagnostic::new(
                        "E_INVALID_PATH",
                        "a bundle's data must be a plain folder, not a file or symlink",
                    )
                    .file(folder));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io(e).file(folder)),
            }
        }
        mods.push(LoadedMod {
            manifest,
            helpers,
            modules,
            file: path.display().to_string(),
            dir,
            crates,
            data,
        });
    }
    mods.sort_by(|a, b| a.manifest.info.id.cmp(&b.manifest.info.id));
    Ok(mods)
}
/// A path written in a manifest uses `/`. A `\` separates components on Windows only, so the same
/// bundle would work for its author and fail for others.
fn forward_slashes(field: &str, value: &str) -> std::result::Result<(), String> {
    if value.contains('\\') {
        return Err(format!(
            "{field} '{value}' contains '\\'; use '/' as the path separator"
        ));
    }
    Ok(())
}
/// Cargo target names: an ASCII letter or `_`, then letters, digits, `_` or `-`.
pub(crate) fn valid_bin(value: &str) -> bool {
    valid_id(value)
        && value
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
}

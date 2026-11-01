//! Native bundle planning for named hooks, crates/data, and same-bundle dependencies.
//!
//! Planning validates everything here against the pristine workspace; [`stage`] then adds the
//! bundle files to the staged file set and edits the root manifest, the dependents' manifests and
//! `Cargo.lock` so that the stage still builds with `--locked`.
use crate::{
    Diagnostic, Plan, Result,
    diagnostic::io,
    manifest::{self, LoadedMod},
    source,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub const CATALOG: &str = "native-hooks.json";
pub const CRATES: &str = "native-crates";
pub const DATA: &str = "native-data";
pub const POST_BUILD: &str = "native-post-build.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedCrate {
    pub mod_id: String,
    pub crate_id: String,
    /// The Cargo package name from the crate's own manifest.
    pub package: String,
    pub version: String,
    /// Staged folder, relative to the workspace root: `native-crates/<mod>/<crate>`.
    pub path: String,
    pub dependent: String,
    /// The dependent's manifest, relative to the workspace root.
    pub dependent_manifest: String,
    /// The `path` written into the dependent's `[dependencies]`.
    pub dependency_path: String,
    /// The `dependencies` of the crate's `Cargo.lock` entry, as Cargo writes them.
    pub lock_dependencies: Vec<String>,
    /// Same-bundle path dependencies, resolved to declared crate IDs in manifest/table order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<PlannedCrateDependency>,
    /// File (relative to the crate folder) → SHA-256.
    pub files: BTreeMap<String, String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedCrateDependency {
    pub table: String,
    pub key: String,
    pub target_crate_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedData {
    pub mod_id: String,
    /// File (relative to the bundle's `data/`) → SHA-256.
    pub files: BTreeMap<String, String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedPostBuild {
    pub mod_id: String,
    pub crate_id: String,
    pub package: String,
    pub bin: String,
    pub args: Vec<String>,
}

#[derive(Deserialize)]
struct Catalog {
    hooks: Vec<CatalogHook>,
}
#[derive(Deserialize)]
struct CatalogHook {
    name: String,
    version: u32,
    target: String,
}

/// Replace every `(hook, hook_version)` with the catalog's target.
pub fn resolve_hooks(root: &Path, mods: &mut [LoadedMod]) -> Result<()> {
    let mut catalog: Option<BTreeMap<String, BTreeMap<u32, String>>> = None;
    for bundle in mods {
        let mod_id = bundle.manifest.info.id.clone();
        let file = bundle.file.clone();
        for injection in &mut bundle.manifest.injection {
            let (Some(hook), Some(version)) = (injection.hook.clone(), injection.hook_version)
            else {
                continue;
            };
            let context = |e: Diagnostic, injection: &manifest::Injection| {
                e.injection(&mod_id, injection).file(&file)
            };
            if catalog.is_none() {
                catalog = Some(load_catalog(root).map_err(|e| context(e, injection))?);
            }
            let hooks = catalog.as_ref().unwrap();
            let Some(versions) = hooks.get(&hook) else {
                let known: Vec<_> = hooks.keys().map(String::as_str).collect();
                return Err(context(
                    Diagnostic::new(
                        "E_HOOK_UNKNOWN",
                        format!(
                            "hook '{hook}' is not in {CATALOG}; known hooks: {}",
                            known.join(", ")
                        ),
                    ),
                    injection,
                ));
            };
            let Some(target) = versions.get(&version) else {
                let known: Vec<_> = versions.keys().map(|v| v.to_string()).collect();
                return Err(context(
                    Diagnostic::new(
                        "E_HOOK_VERSION",
                        format!(
                            "hook '{hook}' has no version {version}; this Horizon has version {}",
                            known.join(", ")
                        ),
                    ),
                    injection,
                ));
            };
            injection.target = target.clone();
        }
    }
    Ok(())
}
fn load_catalog(root: &Path) -> Result<BTreeMap<String, BTreeMap<u32, String>>> {
    let path = root.join(CATALOG);
    let missing = || {
        Diagnostic::new(
            "E_HOOK_CATALOG",
            format!("a named hook needs {CATALOG} at the workspace root; this workspace has none"),
        )
        .file(&path)
    };
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Err(missing()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(missing()),
        Err(e) => return Err(io(e).file(&path)),
    }
    let text = std::fs::read_to_string(&path).map_err(|e| io(e).file(&path))?;
    let parsed: Catalog = serde_json::from_str(&text).map_err(|e| {
        Diagnostic::new("E_HOOK_CATALOG", format!("invalid {CATALOG}: {e}")).file(&path)
    })?;
    let mut hooks: BTreeMap<String, BTreeMap<u32, String>> = BTreeMap::new();
    for hook in parsed.hooks {
        if hook.name.is_empty() || hook.version == 0 || hook.target.is_empty() {
            return Err(Diagnostic::new(
                "E_HOOK_CATALOG",
                format!("{CATALOG} entries need a name, a version above 0 and a target"),
            )
            .file(&path));
        }
        if hooks
            .entry(hook.name.clone())
            .or_default()
            .insert(hook.version, hook.target)
            .is_some()
        {
            return Err(Diagnostic::new(
                "E_HOOK_CATALOG",
                format!("{CATALOG} lists {} v{} twice", hook.name, hook.version),
            )
            .file(&path));
        }
    }
    Ok(hooks)
}

/// The staged bundle files ([`stage`] checks them against the plan's fingerprints).
pub type Extras = BTreeMap<String, Vec<u8>>;

#[derive(Deserialize)]
struct Lock {
    #[serde(default)]
    package: Vec<LockPackage>,
}
#[derive(Deserialize)]
struct LockPackage {
    name: String,
    version: String,
    source: Option<String>,
    checksum: Option<String>,
}

/// Refuse what `cargo update --workspace` added to the stage's lock (`updated`): a registry
/// package that the workspace's own lock (`original`) lacks, or has in another version, source or
/// checksum. The features of a bundle crate's dependencies may only add dependency edges between
/// packages that are already locked; the workspace's lock stays the allow list.
pub fn refuse_new_packages(plan: &Plan, original: &str, updated: &str) -> Result<()> {
    type Locked = (String, String, String, Option<String>);
    let downloaded = |text: &str| -> Result<BTreeSet<Locked>> {
        let lock: Lock = toml::from_str(text).map_err(|e| {
            Diagnostic::new("E_CRATE", format!("cannot read Cargo.lock: {e}")).file("Cargo.lock")
        })?;
        Ok(lock
            .package
            .into_iter()
            .filter_map(|p| Some((p.name, p.version, p.source?, p.checksum)))
            .collect())
    };
    let known = downloaded(original)?;
    let added: Vec<String> = downloaded(updated)?
        .difference(&known)
        .map(|(name, version, ..)| format!("{name} {version}"))
        .collect();
    if added.is_empty() {
        return Ok(());
    }
    let mods: BTreeSet<&str> = plan.crates.iter().map(|c| c.mod_id.as_str()).collect();
    Err(Diagnostic::new(
        "E_CRATE_DEPENDENCY",
        format!(
            "{} needs {}, which {} in Horizon's Cargo.lock: a feature of one of its crates' \
             dependencies turns it on",
            mods.into_iter().collect::<Vec<_>>().join(", "),
            added.join(", "),
            if added.len() == 1 { "isn't" } else { "aren't" }
        ),
    ))
}

pub struct Planned {
    pub crates: Vec<PlannedCrate>,
    pub data: Vec<PlannedData>,
    pub post_build: Vec<PlannedPostBuild>,
    pub extras: Extras,
}

fn crate_error(bundle: &LoadedMod, id: &str, code: &str, message: String) -> Diagnostic {
    let mut e = Diagnostic::new(code, message).file(&bundle.file);
    e.mod_id = Some(bundle.manifest.info.id.clone().into());
    e.injection_id = Some(id.into());
    e
}
fn normalized(name: &str) -> String {
    name.replace('-', "_")
}

pub fn plan(
    root: &Path,
    packages: &BTreeMap<String, String>,
    workspace_bins: &BTreeSet<String>,
    inputs: &BTreeMap<String, String>,
    mods: &[LoadedMod],
) -> Result<Planned> {
    if let Some(reserved) = inputs.keys().find(|name| {
        name.as_str() == POST_BUILD
            || name.starts_with(&format!("{CRATES}/"))
            || name.starts_with(&format!("{DATA}/"))
    }) {
        return Err(Diagnostic::new(
            "E_INVALID_PATH",
            format!("the workspace uses a path horizon-mixins stages into: {reserved}"),
        )
        .file(reserved));
    }
    let mut planned = Planned {
        crates: Vec::new(),
        data: Vec::new(),
        post_build: Vec::new(),
        extras: BTreeMap::new(),
    };
    let needs_lock = mods.iter().any(|m| !m.manifest.crates.is_empty());
    let lock: Lock = if needs_lock {
        let text = std::fs::read_to_string(root.join("Cargo.lock")).map_err(io)?;
        toml::from_str(&text).map_err(|e| {
            Diagnostic::new("E_CRATE", format!("cannot read Cargo.lock: {e}"))
                .file(root.join("Cargo.lock"))
        })?
    } else {
        Lock {
            package: Vec::new(),
        }
    };
    let mut taken: BTreeMap<String, String> = BTreeMap::new();
    for name in packages.keys() {
        taken.insert(normalized(name), format!("workspace member '{name}'"));
    }
    for package in &lock.package {
        taken
            .entry(normalized(&package.name))
            .or_insert_with(|| format!("'{}' in Cargo.lock", package.name));
    }
    let mut dependents: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    // Every bin is built into the same target folder, so a post-build bin's file name (ignoring
    // case, for macOS and Windows) is taken by the workspace's bins and by other crates' steps.
    let mut bin_owners: BTreeMap<String, (String, String)> = BTreeMap::new();
    for bundle in mods {
        let mod_id = &bundle.manifest.info.id;
        let mut packages_by_crate = BTreeMap::new();
        let mut records = Vec::new();
        let mut declared_roots = BTreeMap::new();
        let mut package_versions = BTreeMap::new();
        for (spec, files) in bundle.manifest.crates.iter().zip(&bundle.crates) {
            let fail = |code: &str, message: String| crate_error(bundle, &spec.id, code, message);
            let (source_root, source_relative) =
                checked_crate_root(bundle, spec).map_err(|(code, message)| fail(code, message))?;
            if let Some(owner) = declared_roots.insert(source_root.clone(), spec.id.clone()) {
                return Err(fail(
                    "E_INVALID_PATH",
                    format!(
                        "crates '{owner}' and '{}' resolve to the same physical source root",
                        spec.id
                    ),
                ));
            }
            let info = inspect_crate(files).map_err(|(code, m)| fail(code, m))?;
            let key = normalized(&info.package);
            if let Some(owner) = taken.get(&key) {
                return Err(fail(
                    "E_CRATE",
                    format!(
                        "crate package '{}' collides with {owner}; rename the package",
                        info.package
                    ),
                ));
            }
            taken.insert(key, format!("mod '{mod_id}' crate '{}'", spec.id));
            package_versions.insert(
                spec.id.clone(),
                (
                    info.package.clone(),
                    semver::Version::parse(&info.version).expect("inspect_crate validated version"),
                ),
            );
            let dependent = spec.dependent();
            let Some(dependent_manifest) = packages.get(dependent) else {
                return Err(fail(
                    "E_CRATE",
                    format!(
                        "dependent '{dependent}' is not a workspace member package; members: {}",
                        packages.keys().cloned().collect::<Vec<_>>().join(", ")
                    ),
                ));
            };
            let existing = dependents.entry(dependent.to_string()).or_insert_with(|| {
                std::fs::read_to_string(root.join(dependent_manifest))
                    .ok()
                    .and_then(|t| t.parse::<toml_edit::DocumentMut>().ok())
                    .map(|doc| dependency_names(&doc))
                    .unwrap_or_default()
            });
            if !existing.insert(normalized(&info.package)) {
                return Err(fail(
                    "E_CRATE",
                    format!(
                        "'{dependent}' already has a dependency named '{}'",
                        info.package
                    ),
                ));
            }
            let mut lock_dependencies = BTreeMap::new();
            for (name, req) in &info.registry_dependencies {
                let found = lock
                    .package
                    .iter()
                    .filter(|p| {
                        p.name == *name
                            && p.source.as_deref().is_some_and(|s| {
                                s.starts_with("registry+") || s.starts_with("sparse+")
                            })
                    })
                    .filter_map(|p| semver::Version::parse(&p.version).ok())
                    .filter(|v| req.matches(v))
                    .max();
                let Some(version) = found else {
                    return Err(fail(
                        "E_CRATE_DEPENDENCY",
                        format!("{mod_id} needs {name} {req}, which isn't in Horizon's Cargo.lock"),
                    ));
                };
                let ambiguous = lock.package.iter().filter(|p| p.name == *name).count() > 1;
                let text = if ambiguous {
                    format!("{name} {version}")
                } else {
                    name.clone()
                };
                lock_dependencies.insert((name.clone(), version), text);
            }
            let path = format!("{CRATES}/{mod_id}/{}", spec.id);
            let depth = Path::new(dependent_manifest)
                .parent()
                .map_or(0, |p| p.components().count());
            let dependency_path = format!("{}{path}", "../".repeat(depth));
            for (name, bytes) in files {
                planned
                    .extras
                    .insert(format!("{path}/{name}"), bytes.clone());
            }
            packages_by_crate.insert(spec.id.as_str(), (info.package.clone(), info.bins));
            records.push(CrateRecord {
                source_relative,
                path_dependencies: info.path_dependencies,
                lock_dependencies,
                planned: PlannedCrate {
                    mod_id: mod_id.clone(),
                    crate_id: spec.id.clone(),
                    package: info.package,
                    version: info.version,
                    path,
                    dependent: dependent.to_string(),
                    dependent_manifest: dependent_manifest.clone(),
                    dependency_path,
                    lock_dependencies: Vec::new(),
                    dependencies: Vec::new(),
                    files: fingerprints(files),
                },
            });
        }
        for record in &mut records {
            let bundle_root = bundle.dir.as_ref().ok_or_else(|| {
                crate_error(
                    bundle,
                    "bundle",
                    "E_CRATE",
                    "bundle crates need a bundle directory".into(),
                )
            })?;
            let crate_id = record.planned.crate_id.clone();
            for dependency in &record.path_dependencies {
                let fail = |message: String| {
                    crate_error(
                        bundle,
                        &crate_id,
                        "E_CRATE_DEPENDENCY",
                        format!("[{}] {}: {message}", dependency.table, dependency.key),
                    )
                };
                let target_root =
                    resolve_sibling_path(bundle_root, &record.source_relative, &dependency.path)
                        .map_err(fail)?;
                let Some(target_id) = declared_roots.get(&target_root) else {
                    return Err(fail(format!(
                        "path '{}' does not identify another declared crate root in this bundle",
                        dependency.path
                    )));
                };
                if target_id == &crate_id {
                    return Err(fail("a crate cannot depend on itself".into()));
                }
                let (target_package, target_version) = &package_versions[target_id];
                let declared_package = dependency.package.as_deref().unwrap_or(&dependency.key);
                if declared_package != target_package {
                    return Err(fail(format!(
                        "path '{}' resolves to crate '{}' with package '{}', but the dependency names package '{}'",
                        dependency.path, target_id, target_package, declared_package
                    )));
                }
                if let Some(version) = &dependency.version
                    && !version.matches(target_version)
                {
                    return Err(fail(format!(
                        "path '{}' resolves to package '{}' version {}, which does not match version requirement '{}'",
                        dependency.path, target_package, target_version, version
                    )));
                }
                let edge = PlannedCrateDependency {
                    table: dependency.table.clone(),
                    key: dependency.key.clone(),
                    target_crate_id: target_id.clone(),
                };
                record.lock_dependencies.insert(
                    (target_package.clone(), target_version.clone()),
                    target_package.clone(),
                );
                record.planned.dependencies.push(edge);
            }
            record.planned.lock_dependencies = std::mem::take(&mut record.lock_dependencies)
                .into_values()
                .collect();
        }
        validate_crate_cycles(&records, bundle)?;
        planned
            .crates
            .extend(records.into_iter().map(|record| record.planned));
        if !bundle.data.is_empty() {
            for (name, bytes) in &bundle.data {
                planned
                    .extras
                    .insert(format!("{DATA}/{mod_id}/{name}"), bytes.clone());
            }
            planned.data.push(PlannedData {
                mod_id: mod_id.clone(),
                files: fingerprints(&bundle.data),
            });
        }
        for post in &bundle.manifest.post_build {
            let (package, bins) = &packages_by_crate[post.crate_id.as_str()];
            if !bins.contains(&post.bin) {
                return Err(crate_error(
                    bundle,
                    &post.crate_id,
                    "E_POST_BUILD",
                    format!(
                        "crate '{}' ({package}) has no bin '{}'; it has: {}",
                        post.crate_id,
                        post.bin,
                        if bins.is_empty() {
                            "none".into()
                        } else {
                            bins.iter().cloned().collect::<Vec<_>>().join(", ")
                        }
                    ),
                ));
            }
            let file = post.bin.to_lowercase();
            let clash = |owner: String| {
                crate_error(
                    bundle,
                    &post.crate_id,
                    "E_POST_BUILD",
                    format!(
                        "post_build bin '{}' has the file name of {owner}; every bin is built \
                         into the same target folder, so rename the bin",
                        post.bin
                    ),
                )
            };
            if let Some(bin) = workspace_bins.iter().find(|b| b.to_lowercase() == file) {
                return Err(clash(format!("the workspace's bin '{bin}'")));
            }
            let owner = (mod_id.clone(), post.crate_id.clone());
            match bin_owners.get(&file) {
                Some(other) if *other != owner => {
                    return Err(clash(format!(
                        "the post-build bin of mod '{}' (crate '{}')",
                        other.0, other.1
                    )));
                }
                _ => {
                    bin_owners.insert(file, owner);
                }
            }
            planned.post_build.push(PlannedPostBuild {
                mod_id: mod_id.clone(),
                crate_id: post.crate_id.clone(),
                package: package.clone(),
                bin: post.bin.clone(),
                args: post.args.clone(),
            });
        }
    }
    Ok(planned)
}
fn fingerprints(files: &BTreeMap<String, Vec<u8>>) -> BTreeMap<String, String> {
    files
        .iter()
        .map(|(name, bytes)| (name.clone(), source::fingerprint(bytes)))
        .collect()
}
const DEPENDENCY_TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
fn dependency_names(doc: &toml_edit::DocumentMut) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut add = |table: &dyn toml_edit::TableLike| {
        for kind in DEPENDENCY_TABLES {
            if let Some(deps) = table.get(kind).and_then(|d| d.as_table_like()) {
                for (name, value) in deps.iter() {
                    names.insert(normalized(name));
                    if let Some(package) = value.get("package").and_then(|p| p.as_str()) {
                        names.insert(normalized(package));
                    }
                }
            }
        }
    };
    add(doc.as_table());
    if let Some(targets) = doc.get("target").and_then(|t| t.as_table_like()) {
        for (_, target) in targets.iter() {
            if let Some(target) = target.as_table_like() {
                add(target);
            }
        }
    }
    names
}

struct CrateRecord {
    source_relative: Vec<String>,
    path_dependencies: Vec<PathDependency>,
    lock_dependencies: BTreeMap<(String, semver::Version), String>,
    planned: PlannedCrate,
}
struct PathDependency {
    table: String,
    key: String,
    path: String,
    package: Option<String>,
    version: Option<semver::VersionReq>,
}
struct CrateInfo {
    package: String,
    version: String,
    registry_dependencies: Vec<(String, semver::VersionReq)>,
    path_dependencies: Vec<PathDependency>,
    bins: BTreeSet<String>,
}
type Invalid = (&'static str, String);
fn parse_path_dependency(
    table: &str,
    key: &str,
    spec: &toml::Table,
) -> std::result::Result<PathDependency, Invalid> {
    let dependency =
        |message: String| ("E_CRATE_DEPENDENCY", format!("[{table}] {key}: {message}"));
    if let Some(key) = spec.keys().find(|k| {
        !matches!(
            k.as_str(),
            "path" | "package" | "version" | "features" | "default-features" | "optional"
        )
    }) {
        return Err(dependency(format!(
            "'{key}' is not allowed for a same-bundle path dependency"
        )));
    }
    let path = spec
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| dependency("'path' must be a string".into()))?;
    validate_path_syntax(path, true).map_err(dependency)?;
    let package = match spec.get("package") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or_else(|| dependency("'package' must be a string".into()))?
                .to_string(),
        ),
    };
    let version = match spec.get("version") {
        None => None,
        Some(value) => {
            let text = value
                .as_str()
                .ok_or_else(|| dependency("'version' must be a string".into()))?;
            Some(
                semver::VersionReq::parse(text)
                    .map_err(|e| dependency(format!("version '{text}': {e}")))?,
            )
        }
    };
    if let Some(features) = spec.get("features") {
        let features = features
            .as_array()
            .ok_or_else(|| dependency("'features' must be an array of strings".into()))?;
        if features.iter().any(|v| v.as_str().is_none()) {
            return Err(dependency("'features' must be an array of strings".into()));
        }
    }
    for key in ["default-features", "optional"] {
        if spec.get(key).is_some_and(|v| v.as_bool().is_none()) {
            return Err(dependency(format!("'{key}' must be a boolean")));
        }
    }
    Ok(PathDependency {
        table: table.to_string(),
        key: key.to_string(),
        path: path.to_string(),
        package,
        version,
    })
}

fn inspect_crate(files: &BTreeMap<String, Vec<u8>>) -> std::result::Result<CrateInfo, Invalid> {
    let bad = |m: String| ("E_CRATE", m);
    if files.contains_key("Cargo.lock") {
        return Err(bad(
            "a bundle crate cannot ship a Cargo.lock; it builds against Horizon's".into(),
        ));
    }
    let text = std::str::from_utf8(&files["Cargo.toml"])
        .map_err(|_| bad("Cargo.toml is not UTF-8".into()))?;
    let doc: toml::Table = toml::from_str(text).map_err(|e| bad(format!("Cargo.toml: {e}")))?;
    for key in doc.keys() {
        if !matches!(
            key.as_str(),
            "package"
                | "lib"
                | "bin"
                | "test"
                | "example"
                | "bench"
                | "features"
                | "lints"
                | "badges"
                | "dependencies"
                | "dev-dependencies"
                | "build-dependencies"
        ) {
            return Err(bad(format!(
                "Cargo.toml: [{key}] is not supported in a bundle crate{}",
                match key.as_str() {
                    "workspace" => "; the stage adds it to Horizon's workspace",
                    "target" => "; platform-specific dependencies are unsupported",
                    _ => "",
                }
            )));
        }
    }
    let package = doc
        .get("package")
        .and_then(|p| p.as_table())
        .ok_or_else(|| bad("Cargo.toml needs a [package]".into()))?;
    if package.contains_key("workspace") {
        return Err(bad("package.workspace is not supported".into()));
    }
    let name = package
        .get("name")
        .and_then(|n| n.as_str())
        .filter(|n| manifest::valid_bin(n))
        .ok_or_else(|| bad("package.name must be a plain crate name".into()))?;
    let version = package
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| bad("package.version must be a literal version".into()))?;
    semver::Version::parse(version)
        .map_err(|e| bad(format!("package.version '{version}': {e}")))?;
    if let Some(build) = package.get("build").and_then(|b| b.as_str()) {
        relative_inside(build).map_err(|m| bad(format!("package.build: {m}")))?;
    }
    let mut bins = BTreeSet::new();
    let mut explicit_bins = false;
    for kind in ["lib", "bin", "test", "example", "bench"] {
        let targets: Vec<&toml::Table> = match doc.get(kind) {
            None => Vec::new(),
            Some(toml::Value::Table(t)) => vec![t],
            Some(toml::Value::Array(a)) => a.iter().filter_map(|v| v.as_table()).collect(),
            Some(_) => return Err(bad(format!("[{kind}] has an unexpected shape"))),
        };
        for target in targets {
            if let Some(path) = target.get("path").and_then(|p| p.as_str()) {
                relative_inside(path).map_err(|m| bad(format!("[{kind}] path: {m}")))?;
            }
            if kind == "bin" {
                explicit_bins = true;
                if let Some(bin) = target.get("name").and_then(|n| n.as_str()) {
                    bins.insert(bin.to_string());
                }
            }
        }
    }
    let autobins = package.get("autobins").and_then(|a| a.as_bool()) != Some(false);
    if autobins || !explicit_bins {
        if files.contains_key("src/main.rs") {
            bins.insert(name.to_string());
        }
        for file in files.keys() {
            if let Some(rest) = file.strip_prefix("src/bin/") {
                let bin = rest
                    .strip_suffix(".rs")
                    .filter(|b| !b.contains('/'))
                    .or_else(|| rest.strip_suffix("/main.rs").filter(|b| !b.contains('/')));
                if let Some(bin) = bin {
                    bins.insert(bin.to_string());
                }
            }
        }
    }
    let mut registry_dependencies = Vec::new();
    let mut path_dependencies = Vec::new();
    for kind in DEPENDENCY_TABLES {
        let Some(table) = doc.get(kind) else {
            continue;
        };
        let table = table
            .as_table()
            .ok_or_else(|| bad(format!("[{kind}] must be a table")))?;
        for (dep, value) in table {
            let dependency = |m: String| ("E_CRATE_DEPENDENCY", format!("[{kind}] {dep}: {m}"));
            let req = match value {
                toml::Value::String(req) => req.as_str(),
                toml::Value::Table(spec) => {
                    if spec.contains_key("path") {
                        path_dependencies.push(parse_path_dependency(kind, dep, spec)?);
                        continue;
                    }
                    if let Some(key) = spec.keys().find(|k| {
                        !matches!(
                            k.as_str(),
                            "version" | "features" | "default-features" | "optional"
                        )
                    }) {
                        return Err(dependency(format!(
                            "'{key}' is not allowed; bundle crates take plain registry \
                             dependencies from Horizon's Cargo.lock (version, features, \
                             default-features, optional)"
                        )));
                    }
                    spec.get("version")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| dependency("needs a version requirement".into()))?
                }
                _ => return Err(dependency("must be a version string or a table".into())),
            };
            let req = semver::VersionReq::parse(req)
                .map_err(|e| dependency(format!("version '{req}': {e}")))?;
            registry_dependencies.push((dep.clone(), req));
        }
    }
    Ok(CrateInfo {
        package: name.to_string(),
        version: version.to_string(),
        registry_dependencies,
        path_dependencies,
        bins,
    })
}
fn relative_inside(path: &str) -> std::result::Result<(), String> {
    source::relative(Path::new(path))
        .map_err(|_| format!("'{path}' must be a relative path inside the crate"))
}

fn validate_path_syntax(
    path: &str,
    allow_parent: bool,
) -> std::result::Result<Vec<String>, String> {
    let invalid = || {
        format!(
            "'{path}' must be a relative slash-separated path with valid components{}",
            if allow_parent {
                " that stays inside the bundle"
            } else {
                ""
            }
        )
    };
    if path.is_empty()
        || path.contains('\0')
        || path.contains('\\')
        || path.starts_with('/')
        || path
            .chars()
            .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
    {
        return Err(invalid());
    }
    let parts: Vec<_> = path.split('/').collect();
    if parts.iter().any(|part| {
        part.is_empty()
            || *part == "."
            || (!allow_parent && *part == "..")
            || (*part != ".." && (part.ends_with('.') || part.ends_with(' ')))
    }) {
        return Err(invalid());
    }
    Ok(parts.into_iter().map(str::to_string).collect())
}

fn checked_crate_root(
    bundle: &LoadedMod,
    spec: &manifest::Crate,
) -> std::result::Result<(PathBuf, Vec<String>), Invalid> {
    let parts = validate_path_syntax(&spec.path, false).map_err(|m| ("E_INVALID_PATH", m))?;
    let Some(bundle_root) = &bundle.dir else {
        return Err(("E_CRATE", "bundle crates need a bundle directory".into()));
    };
    let mut current = bundle_root.clone();
    for part in &parts {
        current.push(part);
        let metadata = std::fs::symlink_metadata(&current).map_err(|e| {
            (
                "E_INVALID_PATH",
                format!("crate root '{}' changed during planning: {e}", spec.path),
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err((
                "E_INVALID_PATH",
                format!("crate root '{}' traverses a symlink", spec.path),
            ));
        }
        if !metadata.is_dir() {
            return Err((
                "E_CRATE",
                format!("crate root '{}' is not a directory", spec.path),
            ));
        }
    }
    let physical = current.canonicalize().map_err(|e| {
        (
            "E_INVALID_PATH",
            format!("cannot resolve crate root '{}': {e}", spec.path),
        )
    })?;
    if !physical.starts_with(bundle_root) {
        return Err((
            "E_INVALID_PATH",
            format!("crate root '{}' escapes its bundle", spec.path),
        ));
    }
    Ok((physical, parts))
}

fn resolve_sibling_path(
    bundle_root: &Path,
    source_root: &[String],
    path: &str,
) -> std::result::Result<PathBuf, String> {
    let parts = validate_path_syntax(path, true)?;
    let join = |parts: &[String]| {
        let mut path = bundle_root.to_path_buf();
        for part in parts {
            path.push(part);
        }
        path
    };
    let mut resolved = source_root.to_vec();
    for part in parts {
        if part == ".." {
            if resolved.pop().is_none() {
                return Err(format!("path '{path}' escapes its bundle"));
            }
        } else {
            resolved.push(part);
            let traversed = join(&resolved);
            let metadata = std::fs::symlink_metadata(&traversed)
                .map_err(|_| format!("path '{path}' does not exist inside its bundle"))?;
            if metadata.file_type().is_symlink() {
                return Err(format!("path '{path}' traverses a symlink"));
            }
            if !metadata.is_dir() {
                return Err(format!("path '{path}' traverses a non-directory component"));
            }
        }
    }
    if resolved.is_empty() {
        return Err(format!("path '{path}' does not resolve to a crate folder"));
    }
    let physical = join(&resolved)
        .canonicalize()
        .map_err(|_| format!("path '{path}' does not resolve inside its bundle"))?;
    if !physical.starts_with(bundle_root) {
        return Err(format!("path '{path}' escapes its bundle"));
    }
    Ok(physical)
}

fn cycle_chain(crates: &[&PlannedCrate]) -> Option<Vec<String>> {
    fn visit(
        at: usize,
        crates: &[&PlannedCrate],
        by_id: &BTreeMap<&str, usize>,
        marks: &mut [u8],
        stack: &mut Vec<usize>,
    ) -> Option<Vec<String>> {
        marks[at] = 1;
        stack.push(at);
        for edge in &crates[at].dependencies {
            let Some(&next) = by_id.get(edge.target_crate_id.as_str()) else {
                continue;
            };
            if marks[next] == 1 {
                let start = stack.iter().position(|&index| index == next).unwrap();
                let mut cycle: Vec<_> = stack[start..]
                    .iter()
                    .map(|&index| crates[index].crate_id.clone())
                    .collect();
                cycle.push(crates[next].crate_id.clone());
                return Some(cycle);
            }
            if marks[next] == 0
                && let Some(cycle) = visit(next, crates, by_id, marks, stack)
            {
                return Some(cycle);
            }
        }
        stack.pop();
        marks[at] = 2;
        None
    }

    let by_id: BTreeMap<_, _> = crates
        .iter()
        .enumerate()
        .map(|(index, c)| (c.crate_id.as_str(), index))
        .collect();
    let mut marks = vec![0; crates.len()];
    let mut stack = Vec::new();
    for at in 0..crates.len() {
        if marks[at] == 0
            && let Some(cycle) = visit(at, crates, &by_id, &mut marks, &mut stack)
        {
            return Some(cycle);
        }
    }
    None
}

fn validate_crate_cycles(records: &[CrateRecord], bundle: &LoadedMod) -> Result<()> {
    let crates: Vec<_> = records.iter().map(|record| &record.planned).collect();
    if let Some(cycle) = cycle_chain(&crates) {
        let first = cycle.first().map(String::as_str).unwrap_or("crate");
        return Err(crate_error(
            bundle,
            first,
            "E_CRATE_DEPENDENCY",
            format!("same-bundle crate dependency cycle: {}", cycle.join(" -> ")),
        ));
    }
    Ok(())
}

pub fn validate_plan_edges(plan: &Plan) -> Result<()> {
    let mut by_bundle: BTreeMap<&str, Vec<&PlannedCrate>> = BTreeMap::new();
    for c in &plan.crates {
        let expected_path = format!("{CRATES}/{}/{}", c.mod_id, c.crate_id);
        let depth = Path::new(&c.dependent_manifest)
            .parent()
            .map_or(0, |path| path.components().count());
        let expected_dependency_path = format!("{}{expected_path}", "../".repeat(depth));
        if !manifest::valid_id(&c.mod_id)
            || !manifest::valid_id(&c.crate_id)
            || !manifest::valid_bin(&c.package)
            || semver::Version::parse(&c.version).is_err()
            || c.path != expected_path
            || c.dependency_path != expected_dependency_path
            || !plan.inputs.contains_key(&c.dependent_manifest)
        {
            return Err(Diagnostic::new(
                "E_PLAN_SITE",
                format!("invalid planned bundle crate '{}'", c.crate_id),
            ));
        }
        by_bundle.entry(c.mod_id.as_str()).or_default().push(c);
    }
    for (mod_id, crates) in by_bundle {
        let mut by_id = BTreeMap::new();
        for c in &crates {
            if by_id.insert(c.crate_id.as_str(), *c).is_some() {
                return Err(Diagnostic::new("E_PLAN_SITE", "duplicate planned crate ID"));
            }
        }
        for c in &crates {
            let mut edges = BTreeSet::new();
            for edge in &c.dependencies {
                if !DEPENDENCY_TABLES.contains(&edge.table.as_str())
                    || edge.key.is_empty()
                    || !edges.insert((edge.table.as_str(), edge.key.as_str()))
                {
                    return Err(Diagnostic::new(
                        "E_PLAN_SITE",
                        "invalid or duplicate planned crate dependency edge",
                    ));
                }
                let Some(target) = by_id.get(edge.target_crate_id.as_str()) else {
                    return Err(Diagnostic::new(
                        "E_PLAN_SITE",
                        format!(
                            "planned crate '{}' depends on an undeclared crate in mod '{mod_id}'",
                            c.crate_id
                        ),
                    ));
                };
                if target.crate_id.as_str() == c.crate_id.as_str() {
                    return Err(Diagnostic::new(
                        "E_PLAN_SITE",
                        format!("planned crate '{}' depends on itself", c.crate_id),
                    ));
                }
            }
        }
        if let Some(cycle) = cycle_chain(&crates) {
            return Err(Diagnostic::new(
                "E_PLAN_SITE",
                format!("same-bundle crate dependency cycle: {}", cycle.join(" -> ")),
            ));
        }
    }
    Ok(())
}

fn planned_crate_error(c: &PlannedCrate, message: impl Into<String>) -> Diagnostic {
    let manifest = format!("{}/Cargo.toml", c.path);
    let mut error = Diagnostic::new("E_CRATE_DEPENDENCY", message.into()).file(manifest);
    error.mod_id = Some(c.mod_id.clone().into());
    error.injection_id = Some(c.crate_id.clone().into());
    error
}

fn validate_sibling_edges(plan: &Plan, extras: &Extras) -> Result<()> {
    for source_crate in &plan.crates {
        let mut expected = BTreeMap::new();
        for edge in &source_crate.dependencies {
            expected.insert((edge.table.as_str(), edge.key.as_str()), edge);
        }
        let manifest_path = format!("{}/Cargo.toml", source_crate.path);
        let bytes = extras.get(&manifest_path).ok_or_else(|| {
            Diagnostic::new("E_STALE_SOURCE", "bundle crate manifest is missing")
                .file(&manifest_path)
        })?;
        let text = std::str::from_utf8(bytes).map_err(|_| {
            Diagnostic::new("E_CRATE", "Cargo.toml is not UTF-8").file(&manifest_path)
        })?;
        let doc: toml::Table = toml::from_str(text)
            .map_err(|e| Diagnostic::new("E_CRATE", format!("Cargo.toml: {e}")))?;
        let mut found = BTreeSet::new();
        for table_name in DEPENDENCY_TABLES {
            let Some(dependencies) = doc.get(table_name) else {
                continue;
            };
            let dependencies = dependencies.as_table().ok_or_else(|| {
                planned_crate_error(source_crate, format!("[{table_name}] must be a table"))
            })?;
            for (key, value) in dependencies {
                let Some(spec) = value.as_table() else {
                    continue;
                };
                if !spec.contains_key("path") {
                    continue;
                }
                let dependency = parse_path_dependency(table_name, key, spec)
                    .map_err(|(_, message)| planned_crate_error(source_crate, message))?;
                let edge_key = (table_name, key.as_str());
                let Some(edge) = expected.get(&edge_key) else {
                    return Err(planned_crate_error(
                        source_crate,
                        format!(
                            "[{table_name}] {key} has a path dependency missing from the resolved plan"
                        ),
                    ));
                };
                let target = plan
                    .crates
                    .iter()
                    .find(|c| c.mod_id == source_crate.mod_id && c.crate_id == edge.target_crate_id)
                    .ok_or_else(|| {
                        planned_crate_error(
                            source_crate,
                            format!(
                                "[{table_name}] {key} targets undeclared crate '{}'",
                                edge.target_crate_id
                            ),
                        )
                    })?;
                let declared_package = dependency.package.as_deref().unwrap_or(key);
                if declared_package != target.package {
                    return Err(planned_crate_error(
                        source_crate,
                        format!(
                            "[{table_name}] {key} names package '{declared_package}', but planned target '{}' is package '{}'",
                            target.crate_id, target.package
                        ),
                    ));
                }
                if let Some(version) = dependency.version {
                    let target_version = semver::Version::parse(&target.version).map_err(|_| {
                        planned_crate_error(
                            source_crate,
                            format!(
                                "planned target '{}' has an invalid version",
                                target.crate_id
                            ),
                        )
                    })?;
                    if !version.matches(&target_version) {
                        return Err(planned_crate_error(
                            source_crate,
                            format!(
                                "[{table_name}] {key} version requirement '{version}' does not match target '{}' version {}",
                                target.crate_id, target.version
                            ),
                        ));
                    }
                }
                if !found.insert(edge_key) {
                    return Err(planned_crate_error(
                        source_crate,
                        format!("[{table_name}] {key} repeats a path dependency"),
                    ));
                }
            }
        }
        let expected_keys: BTreeSet<_> = expected.keys().copied().collect();
        if found != expected_keys {
            let missing: Vec<_> = expected_keys
                .difference(&found)
                .map(|(table, key)| format!("[{table}] {key}"))
                .collect();
            return Err(planned_crate_error(
                source_crate,
                format!(
                    "resolved path dependencies no longer match Cargo.toml{}",
                    if missing.is_empty() {
                        String::new()
                    } else {
                        format!("; missing {}", missing.join(", "))
                    }
                ),
            ));
        }
    }
    Ok(())
}

/// Add the planned bundle files to `files` (the transformed workspace) and edit the manifests
/// and `Cargo.lock`. Always writes `native-post-build.json`.
pub fn stage(plan: &Plan, files: &mut BTreeMap<String, Vec<u8>>, extras: Extras) -> Result<()> {
    plan.validate_structure()?;
    let mut expected = BTreeMap::new();
    for c in &plan.crates {
        for (name, hash) in &c.files {
            expected.insert(format!("{}/{name}", c.path), hash);
        }
    }
    for d in &plan.data {
        for (name, hash) in &d.files {
            expected.insert(format!("{DATA}/{}/{name}", d.mod_id), hash);
        }
    }
    if expected.len() != extras.len()
        || extras
            .iter()
            .any(|(name, bytes)| expected.get(name) != Some(&&source::fingerprint(bytes)))
    {
        return Err(Diagnostic::new(
            "E_STALE_SOURCE",
            "bundle crate or data files changed during planning; retry",
        ));
    }
    validate_sibling_edges(plan, &extras)?;
    for (name, bytes) in extras {
        source::relative(Path::new(&name))?;
        if files.insert(name.clone(), bytes).is_some() {
            return Err(Diagnostic::new(
                "E_INVALID_PATH",
                "a staged bundle file overwrites a workspace file",
            )
            .file(name));
        }
    }
    if !plan.crates.is_empty() {
        edit_manifests(plan, files)?;
        let lock = files
            .get("Cargo.lock")
            .ok_or_else(|| Diagnostic::new("E_UNPINNED", "workspace has no Cargo.lock"))?;
        let lock = std::str::from_utf8(lock)
            .map_err(|_| Diagnostic::new("E_CRATE", "Cargo.lock is not UTF-8"))?;
        let edited = edit_lock(lock, &plan.crates)?;
        files.insert("Cargo.lock".into(), edited.into_bytes());
    }
    #[derive(Serialize)]
    struct Entry<'a> {
        #[serde(rename = "mod")]
        mod_id: &'a str,
        package: &'a str,
        bin: &'a str,
        args: &'a [String],
    }
    let entries: Vec<_> = plan
        .post_build
        .iter()
        .map(|p| Entry {
            mod_id: &p.mod_id,
            package: &p.package,
            bin: &p.bin,
            args: &p.args,
        })
        .collect();
    let mut json = serde_json::to_vec_pretty(&entries)
        .map_err(|e| Diagnostic::new("E_JSON", e.to_string()))?;
    json.push(b'\n');
    files.insert(POST_BUILD.into(), json);
    Ok(())
}
fn parse_manifest(files: &BTreeMap<String, Vec<u8>>, name: &str) -> Result<toml_edit::DocumentMut> {
    let bytes = files
        .get(name)
        .ok_or_else(|| Diagnostic::new("E_CRATE", "manifest is missing").file(name))?;
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|t| t.parse().ok())
        .ok_or_else(|| Diagnostic::new("E_CRATE", "manifest is not valid TOML").file(name))
}
fn edit_manifests(plan: &Plan, files: &mut BTreeMap<String, Vec<u8>>) -> Result<()> {
    let mut root = parse_manifest(files, "Cargo.toml")?;
    let members = root
        .get_mut("workspace")
        .and_then(|w| w.as_table_like_mut())
        .ok_or_else(|| {
            Diagnostic::new("E_CRATE", "the workspace root needs a [workspace] table")
                .file("Cargo.toml")
        })?
        .entry("members")
        .or_insert(toml_edit::value(toml_edit::Array::new()))
        .as_array_mut()
        .ok_or_else(|| {
            Diagnostic::new("E_CRATE", "workspace.members must be an array").file("Cargo.toml")
        })?;
    let decor = members
        .iter()
        .last()
        .map(|v| v.decor().prefix().cloned())
        .unwrap_or_default();
    for c in &plan.crates {
        let mut value = toml_edit::Value::from(c.path.as_str());
        if let Some(prefix) = &decor {
            value.decor_mut().set_prefix(prefix.clone());
        }
        members.push_formatted(value);
    }
    files.insert("Cargo.toml".into(), root.to_string().into_bytes());
    let mut by_manifest: BTreeMap<&str, Vec<&PlannedCrate>> = BTreeMap::new();
    for c in &plan.crates {
        by_manifest
            .entry(c.dependent_manifest.as_str())
            .or_default()
            .push(c);
    }
    for (name, crates) in by_manifest {
        // The dependent may be the root package itself, already edited above.
        let mut doc = parse_manifest(files, name)?;
        let deps = doc
            .entry("dependencies")
            .or_insert(toml_edit::table())
            .as_table_like_mut()
            .ok_or_else(|| {
                Diagnostic::new("E_CRATE", "[dependencies] must be a table").file(name)
            })?;
        for c in crates {
            let mut spec = toml_edit::InlineTable::new();
            spec.insert("path", c.dependency_path.as_str().into());
            deps.insert(&c.package, toml_edit::value(spec));
        }
        files.insert(name.into(), doc.to_string().into_bytes());
    }
    edit_sibling_manifests(plan, files)?;
    Ok(())
}
fn edit_sibling_manifests(plan: &Plan, files: &mut BTreeMap<String, Vec<u8>>) -> Result<()> {
    for source_crate in &plan.crates {
        if source_crate.dependencies.is_empty() {
            continue;
        }
        let manifest = format!("{}/Cargo.toml", source_crate.path);
        let mut doc = parse_manifest(files, &manifest)?;
        for edge in &source_crate.dependencies {
            let target = plan
                .crates
                .iter()
                .find(|c| c.mod_id == source_crate.mod_id && c.crate_id == edge.target_crate_id)
                .ok_or_else(|| {
                    planned_crate_error(
                        source_crate,
                        format!(
                            "resolved target crate '{}' is missing",
                            edge.target_crate_id
                        ),
                    )
                })?;
            let dependency = doc
                .get_mut(&edge.table)
                .and_then(|item| item.as_table_like_mut())
                .and_then(|table| table.get_mut(&edge.key))
                .ok_or_else(|| {
                    planned_crate_error(
                        source_crate,
                        format!(
                            "resolved dependency [{}] {} is missing",
                            edge.table, edge.key
                        ),
                    )
                })?;
            let path = dependency
                .as_table_like_mut()
                .and_then(|spec| spec.get_mut("path"))
                .and_then(|item| item.as_value_mut())
                .ok_or_else(|| {
                    planned_crate_error(
                        source_crate,
                        format!(
                            "resolved dependency [{}] {} has no path value",
                            edge.table, edge.key
                        ),
                    )
                })?;
            let decor = path.decor().clone();
            let mut replacement = toml_edit::Value::from(format!("../{}", target.crate_id));
            *replacement.decor_mut() = decor;
            *path = replacement;
        }
        files.insert(manifest.clone(), doc.to_string().into_bytes());
    }
    Ok(())
}
/// Insert the crates' packages in Cargo's order and format, and list them in their dependents'
/// entries, leaving every other byte of the lock as it was.
fn edit_lock(lock: &str, crates: &[PlannedCrate]) -> Result<String> {
    // A CRLF checkout (Git for Windows' core.autocrlf): edit it with LF, then give its endings back.
    if lock.contains("\r\n") {
        return Ok(edit_lock(&lock.replace("\r\n", "\n"), crates)?.replace('\n', "\r\n"));
    }
    let bad = |m: &str| Diagnostic::new("E_CRATE", m.to_string()).file("Cargo.lock");
    let mut chunks: Vec<String> = lock.split("\n\n").map(str::to_string).collect();
    let field = |block: &str, key: &str| {
        block.lines().find_map(|l| {
            l.strip_prefix(key)?
                .strip_prefix(" = \"")?
                .strip_suffix('"')
                .map(str::to_string)
        })
    };
    let key = |block: &str| {
        let name = field(block, "name")?;
        let version = semver::Version::parse(&field(block, "version")?).ok()?;
        Some((name, version))
    };
    for c in crates {
        let dependent = chunks
            .iter()
            .position(|b| {
                b.starts_with("[[package]]")
                    && field(b, "name").as_deref() == Some(c.dependent.as_str())
                    && field(b, "source").is_none()
            })
            .ok_or_else(|| bad("the dependent has no workspace entry in Cargo.lock"))?;
        chunks[dependent] = add_lock_dependency(&chunks[dependent], &c.package);
        let mut block = format!(
            "[[package]]\nname = \"{}\"\nversion = \"{}\"",
            c.package, c.version
        );
        if !c.lock_dependencies.is_empty() {
            block.push_str("\ndependencies = [\n");
            for dep in &c.lock_dependencies {
                block.push_str(&format!(" \"{dep}\",\n"));
            }
            block.push(']');
        }
        let new_key = (
            c.package.clone(),
            semver::Version::parse(&c.version).unwrap(),
        );
        let at = chunks
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, b)| key(b).is_some_and(|k| k > new_key))
            .map_or(chunks.len(), |(i, _)| i);
        if at == chunks.len() {
            // The last chunk ends with the file's final newline.
            let last = chunks.last_mut().unwrap();
            if let Some(trimmed) = last.strip_suffix('\n') {
                *last = trimmed.to_string();
            }
            chunks.push(format!("{block}\n"));
        } else {
            chunks.insert(at, block);
        }
    }
    Ok(chunks.join("\n\n"))
}
fn add_lock_dependency(block: &str, package: &str) -> String {
    let entry = format!(" \"{package}\",");
    let mut lines: Vec<String> = block.lines().map(str::to_string).collect();
    let trailing = block.ends_with('\n');
    if let Some(start) = lines.iter().position(|l| l == "dependencies = [") {
        let end = start
            + lines[start..]
                .iter()
                .position(|l| l == "]")
                .unwrap_or(lines.len() - start);
        let at = (start + 1..end)
            .find(|&i| {
                let name = lines[i].trim().trim_matches(|c| c == '"' || c == ',');
                name.split(' ').next().unwrap_or(name) > package
            })
            .unwrap_or(end);
        lines.insert(at, entry);
    } else {
        lines.push("dependencies = [".into());
        lines.push(entry);
        lines.push("]".into());
    }
    let mut out = lines.join("\n");
    if trailing {
        out.push('\n');
    }
    out
}

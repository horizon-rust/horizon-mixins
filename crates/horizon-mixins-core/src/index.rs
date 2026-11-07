use crate::{Diagnostic, Result, diagnostic::io, source};
use quote::ToTokens;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use syn::{Attribute, Block, FnArg, Item, Signature};

#[derive(Debug, Clone)]
pub struct Function {
    pub id: String,
    pub module: String,
    pub file: String,
    /// Item indices, descending through inline modules and ending at a function or impl method.
    pub node: Vec<usize>,
    pub signature: Signature,
    pub block: Block,
    pub unsupported_cfg: bool,
    pub unsupported_macro: bool,
    pub generic_impl: bool,
    pub self_type: Option<syn::Type>,
}
#[derive(Debug)]
pub struct Index {
    /// A crate's own names (`extern crate self as horizon_client;`) → the cargo target prefix
    /// its items are indexed under.
    pub(crate) crate_aliases: BTreeMap<String, BTreeSet<String>>,
    pub(crate) standard_macro_aliases: BTreeMap<String, BTreeSet<String>>,
    pub functions: Vec<Function>,
    pub types: Vec<TypeDefinition>,
    pub blocked: BTreeSet<String>,
    pub macro_modules: BTreeSet<String>,
    pub files: BTreeMap<String, syn::File>,
    pub source_owners: BTreeMap<String, BTreeSet<String>>,
    /// Workspace member package name → its manifest, relative to the workspace root.
    pub packages: BTreeMap<String, String>,
    /// The workspace members' bin target names.
    pub bins: BTreeSet<String>,
}
#[derive(Debug, Clone)]
pub struct TypeDefinition {
    pub id: String,
    pub module: String,
    pub file: String,
    pub node: Vec<usize>,
    pub item: syn::ItemStruct,
    pub unsupported_cfg: bool,
    pub unsupported_macro: bool,
}

pub fn tokens(value: &impl ToTokens) -> String {
    value.to_token_stream().to_string()
}
pub fn cfg(attrs: &[Attribute]) -> bool {
    attrs
        .iter()
        .any(|a| a.path().is_ident("cfg") || a.path().is_ident("cfg_attr"))
}

#[derive(PartialEq, Eq)]
pub(crate) enum CfgStatus {
    Active,
    Inactive,
    Unknown,
}

pub(crate) fn cfg_status(attrs: &[Attribute]) -> CfgStatus {
    let mut status = CfgStatus::Active;
    for attr in attrs {
        if attr.path().is_ident("cfg_attr") {
            status = CfgStatus::Unknown;
        } else if attr.path().is_ident("cfg") {
            let predicate = attr.parse_args::<syn::Meta>();
            match predicate {
                Ok(syn::Meta::Path(path)) if path.is_ident("test") => return CfgStatus::Inactive,
                Ok(syn::Meta::List(list))
                    if list.path.is_ident("not")
                        && list
                            .parse_args::<syn::Path>()
                            .is_ok_and(|path| path.is_ident("test")) => {}
                _ => status = CfgStatus::Unknown,
            }
        }
    }
    status
}

/// The compiler's own crate-level attributes. Written as inner attributes (`#![recursion_limit =
/// "256"]`) they configure the compilation and expand nothing, so a root module that carries them
/// is no macro module. Anything that can expand code or bring macros in (`macro_use`, `derive`,
/// `test`, `proc_macro*`, an attribute macro of a crate) is not here, nor is `no_implicit_prelude`,
/// which changes what names mean.
const CRATE_ATTRIBUTES: [&str; 9] = [
    "recursion_limit",
    "type_length_limit",
    "crate_name",
    "crate_type",
    "no_std",
    "no_main",
    "no_builtins",
    "windows_subsystem",
    "feature",
];

/// An attribute of a tool the compiler knows by name: `rustfmt::skip`, `clippy::too_many_arguments`,
/// `diagnostic::on_unimplemented`, `rust_analyzer::skip`. They hint to the tool and expand nothing.
/// Any other path with two segments is another crate's attribute macro.
fn tool_attribute(path: &syn::Path) -> bool {
    let names: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
    path.leading_colon.is_none()
        && match names.as_slice() {
            [tool, name] if tool == "rust_analyzer" => name == "skip",
            [tool, _, ..] => matches!(tool.as_str(), "rustfmt" | "clippy" | "diagnostic"),
            _ => false,
        }
}

/// Whether `attrs` hold an attribute that may expand code, so what the item holds cannot be
/// trusted to be what is written. Only attributes known to expand nothing pass.
pub fn macro_attributes(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        let Some(name) = a.path().get_ident() else {
            return !tool_attribute(a.path());
        };
        let name = name.to_string();
        let inner = matches!(a.style, syn::AttrStyle::Inner(_));
        !(matches!(
            name.as_str(),
            "doc"
                | "cfg"
                | "cfg_attr"
                | "path"
                | "allow"
                | "warn"
                | "deny"
                | "forbid"
                | "expect"
                | "inline"
                | "cold"
                | "must_use"
                | "deprecated"
                | "track_caller"
        ) || (inner && CRATE_ATTRIBUTES.contains(&name.as_str())))
    })
}

impl Index {
    pub fn load(root: &Path) -> Result<Self> {
        // Cargo canonicalizes what it reports (`\\?\C:\…` on Windows, a resolved junction on
        // macOS), so the root is compared and stripped in the same form.
        let root = root.canonicalize().map_err(io)?;
        let root = root.as_path();
        if !root.join("Cargo.lock").is_file() {
            return Err(Diagnostic::new(
                "E_UNPINNED",
                "workspace needs a committed Cargo.lock; generate it before planning",
            )
            .file(root));
        }
        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(root.join("Cargo.toml"))
            .no_deps()
            .other_options(vec!["--offline".into(), "--locked".into()])
            .exec()
            .map_err(|e| Diagnostic::new("E_METADATA", e.to_string()))?;
        if metadata
            .workspace_root
            .as_std_path()
            .canonicalize()
            .map_err(io)?
            != root
        {
            return Err(Diagnostic::new(
                "E_INVALID_PATH",
                "--workspace must name the Cargo workspace root",
            ));
        }
        for package in &metadata.packages {
            for dep in &package.dependencies {
                if let Some(path) = &dep.path
                    && !path
                        .as_std_path()
                        .canonicalize()
                        .map_err(io)?
                        .starts_with(root)
                {
                    return Err(Diagnostic::new(
                        "E_INVALID_PATH",
                        "path dependencies must remain inside the staged workspace",
                    )
                    .file(path.as_std_path()));
                }
            }
        }
        let mut index = Self {
            crate_aliases: BTreeMap::new(),
            standard_macro_aliases: BTreeMap::new(),
            functions: Vec::new(),
            types: Vec::new(),
            blocked: BTreeSet::new(),
            macro_modules: BTreeSet::new(),
            files: BTreeMap::new(),
            source_owners: BTreeMap::new(),
            packages: BTreeMap::new(),
            bins: BTreeSet::new(),
        };
        for package in &metadata.packages {
            if !metadata.workspace_members.contains(&package.id) {
                continue;
            }
            let manifest = package
                .manifest_path
                .as_std_path()
                .canonicalize()
                .map_err(io)?;
            let manifest = manifest
                .strip_prefix(root)
                .map_err(|_| {
                    Diagnostic::new("E_INVALID_PATH", "workspace member outside the workspace")
                })?
                .to_str()
                .ok_or_else(|| Diagnostic::new("E_INVALID_PATH", "non UTF-8 manifest path"))?
                .replace('\\', "/");
            index.packages.insert(package.name.to_string(), manifest);
            for target in &package.targets {
                if target.kind.contains(&cargo_metadata::TargetKind::Bin) {
                    index.bins.insert(target.name.clone());
                }
                if !target.kind.iter().any(|k| {
                    matches!(
                        k,
                        cargo_metadata::TargetKind::Lib
                            | cargo_metadata::TargetKind::Bin
                            | cargo_metadata::TargetKind::RLib
                            | cargo_metadata::TargetKind::CDyLib
                            | cargo_metadata::TargetKind::DyLib
                            | cargo_metadata::TargetKind::StaticLib
                    )
                }) {
                    continue;
                }
                let path = target.src_path.as_std_path();
                let prefix = target.name.replace('-', "_");
                let aliases = package
                    .dependencies
                    .iter()
                    .filter_map(|d| {
                        let alias = d.rename.as_ref().unwrap_or(&d.name).replace('-', "_");
                        matches!(alias.as_str(), "std" | "core" | "alloc").then_some(alias)
                    })
                    .collect();
                index.standard_macro_aliases.insert(prefix.clone(), aliases);
                if !target.required_features.is_empty() {
                    index.blocked.insert(prefix);
                    continue;
                }
                index.read(
                    root,
                    path,
                    &prefix,
                    path.parent().unwrap(),
                    false,
                    &mut BTreeSet::new(),
                )?;
            }
        }
        Ok(index)
    }
    fn read(
        &mut self,
        root: &Path,
        path: &Path,
        prefix: &str,
        module_dir: &Path,
        inherited_cfg: bool,
        active: &mut BTreeSet<PathBuf>,
    ) -> Result<()> {
        let path = path.canonicalize().map_err(|e| io(e).file(path))?;
        if !path.starts_with(root) {
            return Err(Diagnostic::new("E_INVALID_PATH", "module escapes workspace").file(&path));
        }
        if !active.insert(path.clone()) {
            return Err(Diagnostic::new("E_INVALID_PATH", "recursive external module").file(path));
        }
        let file = path
            .strip_prefix(root)
            .unwrap()
            .to_str()
            .ok_or_else(|| Diagnostic::new("E_INVALID_PATH", "non UTF-8 module path"))?
            .replace('\\', "/");
        let tree = syn::parse_file(&fs::read_to_string(&path).map_err(io)?)
            .map_err(|e| Diagnostic::new("E_RUST_SYNTAX", e.to_string()).file(&path))?;
        let file_cfg = cfg_status(&tree.attrs);
        if file_cfg == CfgStatus::Inactive {
            active.remove(&path);
            return Ok(());
        }
        self.items(
            root,
            &tree.items,
            &file,
            prefix,
            module_dir,
            &[],
            inherited_cfg || file_cfg == CfgStatus::Unknown,
            active,
        )?;
        if macro_attributes(&tree.attrs) {
            self.macro_modules.insert(format!("@{prefix}"));
        }
        self.source_owners
            .entry(file.clone())
            .or_default()
            .insert(prefix.into());
        self.files.insert(file, tree);
        active.remove(&path);
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn items(
        &mut self,
        root: &Path,
        items: &[Item],
        file: &str,
        prefix: &str,
        module_dir: &Path,
        parent: &[usize],
        inherited_cfg: bool,
        active: &mut BTreeSet<PathBuf>,
    ) -> Result<()> {
        for (i, item) in items.iter().enumerate() {
            let mut node = parent.to_vec();
            node.push(i);
            match item {
                Item::Fn(f) => {
                    let status = cfg_status(&f.attrs);
                    if status == CfgStatus::Inactive {
                        continue;
                    }
                    self.functions.push(Function {
                        id: format!("{prefix}::{}", f.sig.ident),
                        module: prefix.into(),
                        file: file.into(),
                        node,
                        signature: f.sig.clone(),
                        block: *f.block.clone(),
                        unsupported_cfg: inherited_cfg || status == CfgStatus::Unknown,
                        unsupported_macro: macro_attributes(&f.attrs),
                        generic_impl: false,
                        self_type: None,
                    })
                }
                Item::Mod(m) => {
                    let next = format!("{prefix}::{}", m.ident);
                    let status = cfg_status(&m.attrs);
                    if status == CfgStatus::Inactive {
                        continue;
                    }
                    let uncertain = inherited_cfg || status == CfgStatus::Unknown;
                    if macro_attributes(&m.attrs) {
                        self.macro_modules.insert(format!("@{next}"));
                        continue;
                    }
                    if uncertain {
                        self.blocked.insert(next);
                        continue;
                    }
                    if let Some((_, nested)) = &m.content {
                        self.items(
                            root,
                            nested,
                            file,
                            &next,
                            &module_dir.join(m.ident.to_string()),
                            &node,
                            false,
                            active,
                        )?;
                    } else {
                        let attr = m.attrs.iter().find(|a| a.path().is_ident("path"));
                        let path = if let Some(attr) = attr {
                            let syn::Meta::NameValue(nv) = &attr.meta else {
                                return Err(Diagnostic::new(
                                    "E_INVALID_PATH",
                                    "expected #[path = \"relative.rs\"]",
                                )
                                .file(file));
                            };
                            let syn::Expr::Lit(lit) = &nv.value else {
                                return Err(Diagnostic::new(
                                    "E_INVALID_PATH",
                                    "module path must be literal",
                                )
                                .file(file));
                            };
                            let syn::Lit::Str(value) = &lit.lit else {
                                return Err(Diagnostic::new(
                                    "E_INVALID_PATH",
                                    "module path must be a string",
                                )
                                .file(file));
                            };
                            source::relative(Path::new(&value.value()))?;
                            // #[path] on an external item is relative to the containing source directory.
                            let base = if parent.is_empty() {
                                root.join(file).parent().unwrap().to_path_buf()
                            } else {
                                module_dir.to_path_buf()
                            };
                            base.join(value.value())
                        } else {
                            let a = module_dir.join(format!("{}.rs", m.ident));
                            let b = module_dir.join(m.ident.to_string()).join("mod.rs");
                            match (a.is_file(), b.is_file()) {
                                (true, false) => a,
                                (false, true) => b,
                                (true, true) => {
                                    return Err(Diagnostic::new(
                                        "E_TARGET_AMBIGUOUS",
                                        format!(
                                            "both {} and {} define {next}",
                                            a.display(),
                                            b.display()
                                        ),
                                    )
                                    .file(file));
                                }
                                _ => {
                                    return Err(Diagnostic::new(
                                        "E_INVALID_PATH",
                                        format!("module {next} has no source file"),
                                    )
                                    .file(file));
                                }
                            }
                        };
                        let subdir =
                            if path.file_name().is_some_and(|n| n == "mod.rs") || attr.is_some() {
                                path.parent().unwrap().to_path_buf()
                            } else {
                                path.parent().unwrap().join(path.file_stem().unwrap())
                            };
                        self.read(root, &path, &next, &subdir, false, active)?;
                    }
                }
                Item::Impl(imp) => {
                    let status = cfg_status(&imp.attrs);
                    if status == CfgStatus::Inactive {
                        continue;
                    }
                    let mut identity_type = *imp.self_ty.clone();
                    struct Lifetimes;
                    impl syn::visit_mut::VisitMut for Lifetimes {
                        fn visit_lifetime_mut(&mut self, value: &mut syn::Lifetime) {
                            if value.ident != "static" {
                                *value = syn::parse_quote!('_);
                            }
                        }
                    }
                    syn::visit_mut::VisitMut::visit_type_mut(&mut Lifetimes, &mut identity_type);
                    let ty = tokens(&identity_type).replace(' ', "");
                    let identity = if let Some((_, tr, _)) = &imp.trait_ {
                        format!("<{ty} as {}>", tokens(tr).replace(' ', ""))
                    } else {
                        ty
                    };
                    for (j, member) in imp.items.iter().enumerate() {
                        if let syn::ImplItem::Fn(f) = member {
                            let method_status = cfg_status(&f.attrs);
                            if method_status == CfgStatus::Inactive {
                                continue;
                            }
                            let mut node = node.clone();
                            node.push(j);
                            self.functions.push(Function {
                                id: format!("{prefix}::{identity}::{}", f.sig.ident),
                                module: prefix.into(),
                                file: file.into(),
                                node,
                                signature: f.sig.clone(),
                                block: f.block.clone(),
                                unsupported_cfg: inherited_cfg
                                    || status == CfgStatus::Unknown
                                    || method_status == CfgStatus::Unknown,
                                unsupported_macro: macro_attributes(&imp.attrs)
                                    || macro_attributes(&f.attrs),
                                generic_impl: imp
                                    .generics
                                    .params
                                    .iter()
                                    .any(|p| !matches!(p, syn::GenericParam::Lifetime(_)))
                                    || imp.generics.where_clause.is_some(),
                                self_type: Some(*imp.self_ty.clone()),
                            });
                        }
                    }
                }
                Item::Struct(item) => {
                    let status = cfg_status(&item.attrs);
                    if status == CfgStatus::Inactive {
                        continue;
                    }
                    let attrs: Vec<_> = item
                        .attrs
                        .iter()
                        .filter(|a| {
                            !["derive", "repr", "non_exhaustive"]
                                .iter()
                                .any(|n| a.path().is_ident(n))
                        })
                        .cloned()
                        .collect();
                    self.types.push(TypeDefinition {
                        id: format!("{prefix}::{}", item.ident),
                        module: prefix.into(),
                        file: file.into(),
                        node,
                        item: item.clone(),
                        unsupported_cfg: inherited_cfg || status == CfgStatus::Unknown,
                        unsupported_macro: macro_attributes(&attrs),
                    });
                }
                Item::Macro(_) => {
                    self.macro_modules.insert(prefix.into());
                }
                // The crate's own name: a target may be written with it (`extern crate self as
                // horizon_client;` in a crate cargo calls `horizon-client-lib`).
                Item::ExternCrate(e) if parent.is_empty() => {
                    if e.ident == "self"
                        && let Some((_, name)) = &e.rename
                    {
                        self.crate_aliases
                            .entry(name.to_string())
                            .or_default()
                            .insert(prefix.into());
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    /// The functions a target names. A crate may name itself (`extern crate self as name;`), so
    /// the same path under that name means the crate's own items when the id itself matches none.
    fn matching(&self, target: &str) -> Vec<&Function> {
        let direct: Vec<&Function> = self.functions.iter().filter(|f| f.id == target).collect();
        if !direct.is_empty() {
            return direct;
        }
        let Some((head, rest)) = target.split_once("::") else {
            return direct;
        };
        let candidates: Vec<String> = self
            .crate_aliases
            .get(head)
            .map(|real| real.iter().map(|r| format!("{r}::{rest}")).collect())
            .unwrap_or_default();
        self.functions
            .iter()
            .filter(|f| candidates.contains(&f.id))
            .collect()
    }
    pub fn resolve(&self, target: &str) -> Result<&Function> {
        if self
            .blocked
            .iter()
            .any(|p| target == p || target.starts_with(&format!("{p}::")))
        {
            return Err(Diagnostic::new(
                "E_UNSUPPORTED_CFG",
                "target is under unresolved cfg/cfg_attr; select unconditional source",
            ));
        }
        let found = self.matching(target);
        if self
            .macro_modules
            .iter()
            .filter_map(|p| p.strip_prefix('@'))
            .any(|p| target.starts_with(&format!("{p}::")))
        {
            return Err(Diagnostic::new(
                "E_UNSUPPORTED_MACRO",
                "target module has an unexpanded attribute macro",
            ));
        }
        if let [f] = found.as_slice() {
            if self
                .functions
                .iter()
                .filter(|other| other.file == f.file && other.node == f.node)
                .count()
                > 1
            {
                return Err(Diagnostic::new("E_TARGET_AMBIGUOUS", "source function is reused by multiple modules or Cargo targets; it cannot be patched independently").file(&f.file));
            }
            if f.unsupported_macro {
                return Err(Diagnostic::new(
                    "E_UNSUPPORTED_MACRO",
                    "target function/impl has an unexpanded attribute macro",
                )
                .file(&f.file));
            }
        }
        match found.as_slice() {
            [f] if f.unsupported_cfg => Err(Diagnostic::new(
                "E_UNSUPPORTED_CFG",
                "target or impl has unresolved cfg/cfg_attr",
            )
            .file(&f.file)),
            [f] => Ok(f),
            [] => {
                let module = target.rsplit_once("::").map(|(p, _)| p).unwrap_or(target);
                let code = if self.macro_modules.contains(module) {
                    "E_UNSUPPORTED_MACRO"
                } else {
                    "E_TARGET_NOT_FOUND"
                };
                Err(Diagnostic::new(
                    code,
                    format!(
                        "no source function '{target}'; use an exact crate::module::function, Type::method or <Type as Trait>::method ID"
                    ),
                ))
            }
            _ => Err(Diagnostic::new(
                "E_TARGET_AMBIGUOUS",
                format!(
                    "'{target}' resolves to {} definitions (including Cargo targets); use an unambiguous unconditional target",
                    found.len()
                ),
            )),
        }
    }
}
/// The names a function's parameters bind: `self`, each plain parameter and every name a
/// destructuring pattern such as `(x, z): (f64, f64)` introduces.
pub fn parameters(sig: &Signature) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for arg in &sig.inputs {
        match arg {
            FnArg::Receiver(_) => {
                names.insert("self".to_owned());
            }
            FnArg::Typed(t) => {
                names.extend(pattern_bindings(&t.pat)?.into_iter().map(|(name, _)| name));
            }
        }
    }
    Ok(names)
}
/// Each name a pattern binds, with whether the binding is `mut`, in source order. The parts of
/// a destructured parameter can be named by hooks; the unnamed whole cannot.
pub fn pattern_bindings(pat: &syn::Pat) -> Result<Vec<(String, bool)>> {
    struct Bindings(Vec<(String, bool)>, bool);
    impl<'ast> syn::visit::Visit<'ast> for Bindings {
        fn visit_pat_ident(&mut self, p: &'ast syn::PatIdent) {
            self.0.push((p.ident.to_string(), p.mutability.is_some()));
            syn::visit::visit_pat_ident(self, p);
        }
        fn visit_pat(&mut self, p: &'ast syn::Pat) {
            if matches!(p, syn::Pat::Macro(_)) {
                self.1 = true;
            }
            syn::visit::visit_pat(self, p);
        }
    }
    let mut bindings = Bindings(Vec::new(), false);
    syn::visit::Visit::visit_pat(&mut bindings, pat);
    if bindings.1 {
        return Err(Diagnostic::new(
            "E_INVALID_BINDING",
            "parameter patterns containing macros are unsupported",
        ));
    }
    Ok(bindings.0)
}

#[cfg(test)]
mod tests {
    use super::{CfgStatus, cfg_status, macro_attributes};
    use syn::parse_quote;

    fn inner(source: &str) -> Vec<syn::Attribute> {
        syn::parse_file(source).unwrap().attrs
    }

    #[test]
    fn built_in_crate_attributes_expand_nothing_but_only_as_inner_attributes() {
        for source in [
            "#![recursion_limit = \"256\"]",
            "#![type_length_limit = \"1048576\"]",
            "#![crate_name = \"game\"]",
            "#![crate_type = \"lib\"]",
            "#![no_std]",
            "#![no_main]",
            "#![no_builtins]",
            "#![windows_subsystem = \"windows\"]",
            "#![feature(test)]",
            "#![allow(dead_code)]
#![recursion_limit = \"256\"]",
        ] {
            assert!(!macro_attributes(&inner(source)), "{source}");
        }
        // The known tools' attributes hint to the tool and expand nothing, on any item.
        let tools: [syn::Attribute; 7] = [
            parse_quote!(#[rustfmt::skip]),
            parse_quote!(#[rustfmt::skip::macros(vec)]),
            parse_quote!(#[clippy::cognitive_complexity = "30"]),
            parse_quote!(#[clippy::too_many_arguments]),
            parse_quote!(#[diagnostic::on_unimplemented(message = "no")]),
            parse_quote!(#[diagnostic::do_not_recommend]),
            parse_quote!(#[rust_analyzer::skip]),
        ];
        for attribute in tools {
            assert!(
                !macro_attributes(std::slice::from_ref(&attribute)),
                "{attribute:?}"
            );
        }
        let others: [syn::Attribute; 6] = [
            parse_quote!(#[rust_analyzer::other]),
            parse_quote!(#[serde::something]),
            parse_quote!(#[tokio::main]),
            parse_quote!(#[::rustfmt::skip]),
            parse_quote!(#[::rust_analyzer::skip]),
            parse_quote!(#[rustfmt_helper::skip]),
        ];
        for attribute in others {
            assert!(macro_attributes(&[attribute]));
        }
        // A tool attribute does not hide another one beside it.
        assert!(macro_attributes(&[
            parse_quote!(#[rustfmt::skip]),
            parse_quote!(#[tokio::main]),
        ]));
        // Outer, the same names do not configure the crate; they stay refused.
        assert!(macro_attributes(&[
            parse_quote!(#[recursion_limit = "256"])
        ]));
        assert!(macro_attributes(&[parse_quote!(#[no_std])]));
        // What can expand code or bring macros in stays refused, among accepted ones too.
        for source in [
            "#![macro_use]",
            "#![no_implicit_prelude]",
            "#![some_attribute_macro]",
            "#![tool::attribute]",
            "#![rustfmt_helper::skip]",
            "#![recursion_limit = \"256\"]
#![macro_use]",
        ] {
            assert!(macro_attributes(&inner(source)), "{source}");
        }
    }

    #[test]
    fn production_cfg_recognizes_only_exact_test_predicates() {
        assert!(matches!(
            cfg_status(&[parse_quote!(#[cfg(not(test))])]),
            CfgStatus::Active
        ));
        assert!(matches!(
            cfg_status(&[parse_quote!(#[cfg(test)])]),
            CfgStatus::Inactive
        ));
        assert!(matches!(
            cfg_status(&[parse_quote!(#[cfg(feature = "x")])]),
            CfgStatus::Unknown
        ));
        assert!(matches!(
            cfg_status(&[parse_quote!(#[cfg_attr(test, inline)])]),
            CfgStatus::Unknown
        ));
        assert!(matches!(
            cfg_status(&[parse_quote!(#[cfg(not(any(test)))])]),
            CfgStatus::Unknown
        ));
        assert!(matches!(
            cfg_status(&[
                parse_quote!(#[cfg(feature = "x")]),
                parse_quote!(#[cfg(test)])
            ]),
            CfgStatus::Inactive
        ));
    }
}

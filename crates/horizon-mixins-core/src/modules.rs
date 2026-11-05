use crate::{Diagnostic, Result};
use syn::{
    Item, UseTree,
    visit::{self, Visit},
};

pub fn validate_payload(source: &str) -> Result<syn::File> {
    let file = syn::parse_file(source).map_err(|e| Diagnostic::new("E_MODULE", e.to_string()))?;
    if !file.attrs.is_empty() || file.shebang.is_some() {
        return Err(Diagnostic::new(
            "E_MODULE",
            "support module source cannot have file attributes or a shebang",
        ));
    }
    struct External(bool);
    impl<'ast> Visit<'ast> for External {
        fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
            self.0 |=
                item.content.is_none() || item.attrs.iter().any(|a| a.path().is_ident("path"));
            visit::visit_item_mod(self, item);
        }
        fn visit_macro(&mut self, item: &'ast syn::Macro) {
            self.0 |= ["include", "include_str", "include_bytes"]
                .iter()
                .any(|n| item.path.is_ident(n));
            visit::visit_macro(self, item);
        }
    }
    let mut external = External(false);
    external.visit_file(&file);
    if external.0 {
        return Err(Diagnostic::new(
            "E_MODULE",
            "support modules must be self-contained; external mod/path/include sources are unsupported",
        ));
    }
    Ok(file)
}
/// Whether `tree` imports `name`; a glob import asks `glob` with the path before it.
fn imported(
    tree: &UseTree,
    path: &mut Vec<String>,
    name: &str,
    glob: &mut dyn FnMut(&[String]) -> bool,
) -> bool {
    match tree {
        UseTree::Path(p) => {
            path.push(p.ident.to_string());
            let found = imported(&p.tree, path, name, glob);
            path.pop();
            found
        }
        UseTree::Name(n) => n.ident == name,
        UseTree::Rename(n) => n.rename == name,
        UseTree::Group(g) => g.items.iter().any(|t| imported(t, path, name, glob)),
        UseTree::Glob(_) => glob(path),
    }
}
/// Whether `file`'s top-level items declare or import `name` in the type namespace, where
/// `glob(path)` says whether a glob import of `path` (`["horizon_client_lib"]` for
/// `pub use horizon_client_lib::*;`) may bring it in.
pub fn declares(file: &syn::File, name: &str, glob: &mut dyn FnMut(&[String]) -> bool) -> bool {
    file.items.iter().any(|item| match item {
        Item::Mod(i) => i.ident == name,
        Item::Struct(i) => i.ident == name,
        Item::Enum(i) => i.ident == name,
        Item::Union(i) => i.ident == name,
        Item::Type(i) => i.ident == name,
        Item::Trait(i) => i.ident == name,
        Item::ExternCrate(i) => i.rename.as_ref().map_or(&i.ident, |(_, i)| i) == name,
        Item::Use(i) => imported(&i.tree, &mut Vec::new(), name, glob),
        _ => false,
    })
}
/// A support module's destination must not declare or import its name. Every glob import counts
/// as one: [`validate_destination_with`] follows the globs it can resolve.
pub fn validate_destination(file: &syn::File, name: &str) -> Result<()> {
    validate_destination_with(file, name, &mut |_| true)
}
/// [`validate_destination`], where `glob(path)` says whether a top-level glob import of `path`
/// may bring in `name`: a local module shadows a glob import, so one that cannot is no clash.
pub fn validate_destination_with(
    file: &syn::File,
    name: &str,
    glob: &mut dyn FnMut(&[String]) -> bool,
) -> Result<()> {
    if declares(file, name, glob) {
        return Err(Diagnostic::new(
            "E_MODULE_CONFLICT",
            format!(
                "support module '{name}' collides with an existing type/module/import or unresolved glob import"
            ),
        ));
    }
    Ok(())
}
/// Whether a top-level glob import of `path` in module `from` may bring in `name`: unless it
/// names a module the index holds as a file (`crate::…`, `self::…`, `super::…`, a workspace
/// crate, or a child of `from`) whose own top-level items and globs, followed the same way,
/// neither declare nor import `name` and where no support module `name` is planned (`planned`,
/// as destination file and name), it may.
pub fn glob_may_bring(
    index: &crate::index::Index,
    from: &str,
    path: &[String],
    name: &str,
    planned: &[(&str, &str)],
) -> bool {
    glob_brings(index, from, path, name, planned, 0)
}
fn glob_brings(
    index: &crate::index::Index,
    from: &str,
    path: &[String],
    name: &str,
    planned: &[(&str, &str)],
    depth: usize,
) -> bool {
    let file_of = |module: &str| {
        let mut files = index
            .source_owners
            .iter()
            .filter(|(_, owners)| owners.len() == 1 && owners.contains(module))
            .map(|(file, _)| file.as_str());
        match (files.next(), files.next()) {
            (Some(file), None) => Some(file),
            _ => None,
        }
    };
    let mut module: Vec<&str> = from.split("::").collect();
    let mut rest = path.iter().map(String::as_str).peekable();
    match rest.peek().copied() {
        Some("crate") => {
            module.truncate(1);
            rest.next();
        }
        Some("self") => {
            rest.next();
        }
        Some("super") => {
            while rest.peek() == Some(&"super") {
                rest.next();
                if module.len() < 2 {
                    return true;
                }
                module.pop();
            }
        }
        Some(first)
            if file_of(first).is_some() && file_of(&format!("{from}::{first}")).is_none() =>
        {
            module.clear()
        }
        Some(_) => {}
        None => return true,
    }
    module.extend(rest);
    let target = module.join("::");
    let Some(file) = file_of(&target) else {
        return true;
    };
    if depth > 8 || planned.iter().any(|(f, n)| *f == file && *n == name) {
        return true;
    }
    let Some(tree) = index.files.get(file) else {
        return true;
    };
    declares(tree, name, &mut |inner| {
        glob_brings(index, &target, inner, name, planned, depth + 1)
    })
}

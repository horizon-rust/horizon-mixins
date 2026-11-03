//! Parsed, preserved standard macro inputs. These never become rewrite sites.
use crate::{
    Diagnostic, Result,
    index::{Function, Index, tokens},
    manifest::{Injection, Kind},
    sites,
};
use std::collections::BTreeSet;
use syn::{
    Expr, Item, Macro, Pat, Token, UseTree,
    parse::{ParseStream, Parser},
    punctuated::Punctuated,
    visit::{self, Visit},
};

pub(crate) struct Inputs {
    pub expressions: Vec<Expr>,
    pub pattern: Option<Pat>,
}
impl From<Vec<Expr>> for Inputs {
    fn from(expressions: Vec<Expr>) -> Self {
        Self {
            expressions,
            pattern: None,
        }
    }
}

#[derive(Default)]
pub(crate) struct Policy {
    bare: BTreeSet<String>,
    roots: BTreeSet<String>,
    absolute: BTreeSet<String>,
}
impl Policy {
    pub fn for_function(index: &Index, f: &Function) -> Self {
        let mut policy = Self::default();
        if let Some(aliases) = index
            .standard_macro_aliases
            .get(f.module.split("::").next().unwrap())
        {
            policy.absolute.extend(aliases.iter().cloned());
        }
        for (file, owners) in &index.source_owners {
            for owner in owners {
                if f.module == *owner || f.module.starts_with(&format!("{owner}::")) {
                    policy.scope(&index.files[file].items, owner, &f.module);
                }
            }
        }
        policy
    }
    fn uncertain(&mut self) {
        self.bare
            .extend(["format", "matches", "vec"].map(str::to_owned));
    }
    pub(crate) fn imported(&mut self, tree: &UseTree) {
        match tree {
            UseTree::Path(p) => self.imported(&p.tree),
            UseTree::Name(n) => {
                self.bare.insert(n.ident.to_string());
                self.roots.insert(n.ident.to_string());
            }
            UseTree::Rename(n) => {
                self.bare.insert(n.rename.to_string());
                self.roots.insert(n.rename.to_string());
            }
            UseTree::Group(g) => {
                for t in &g.items {
                    self.imported(t);
                }
            }
            UseTree::Glob(_) => {
                self.uncertain();
                self.roots
                    .extend(["std", "core", "alloc"].map(str::to_owned));
            }
        }
    }
    fn scope(&mut self, items: &[Item], module: &str, target: &str) {
        for item in items {
            match item {
                Item::Macro(m) => {
                    if m.mac.path.is_ident("macro_rules") {
                        if let Some(name) = &m.ident {
                            self.bare.insert(name.to_string());
                        }
                    } else {
                        // An item expansion could introduce imports or macro definitions.
                        self.uncertain();
                        self.absolute
                            .extend(["std", "core", "alloc"].map(str::to_owned));
                    }
                }
                Item::Use(u) if module == target => self.imported(&u.tree),
                Item::ExternCrate(e) => {
                    let name = e.rename.as_ref().map_or(&e.ident, |(_, n)| n).to_string();
                    if e.ident != name {
                        self.absolute.insert(name.clone());
                        self.roots.insert(name);
                    }
                    if e.attrs.iter().any(|a| a.path().is_ident("macro_use")) {
                        self.uncertain();
                    }
                }
                Item::Mod(m) => {
                    self.roots.insert(m.ident.to_string());
                    if m.attrs.iter().any(|a| a.path().is_ident("macro_use")) {
                        self.uncertain();
                    }
                    let next = format!("{module}::{}", m.ident);
                    if (target == next || target.starts_with(&format!("{next}::")))
                        && let Some((_, items)) = &m.content
                    {
                        self.scope(items, &next, target);
                    }
                }
                _ => {}
            }
        }
    }
    pub fn parse(&self, mac: &Macro) -> Result<Inputs> {
        let parts: Vec<_> = mac
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        let name = match parts.as_slice() {
            [name] if !self.bare.contains(name) && mac.path.leading_colon.is_none() => name,
            [root, name]
                if !self.absolute.contains(root)
                    && (mac.path.leading_colon.is_some() || !self.roots.contains(root))
                    && (root == "std"
                        || (root == "core" && name == "matches")
                        || (root == "alloc" && name != "matches")) =>
            {
                name
            }
            _ => {
                return Err(fail(
                    "macro identity is ambiguous; use an unshadowed absolute standard path",
                ));
            }
        };
        let parsed = match name.as_str() {
            "vec" => (|input: ParseStream<'_>| {
                let mut out = Vec::new();
                if !input.is_empty() {
                    out.push(input.parse::<Expr>()?);
                    if input.peek(Token![;]) {
                        input.parse::<Token![;]>()?;
                        out.push(input.parse()?);
                    } else {
                        while !input.is_empty() {
                            input.parse::<Token![,]>()?;
                            if !input.is_empty() {
                                out.push(input.parse()?);
                            }
                        }
                    }
                }
                Ok(Inputs::from(out))
            })
            .parse2(mac.tokens.clone()),
            "format" => (|input: ParseStream<'_>| {
                let args = Punctuated::<Expr, Token![,]>::parse_terminated(input)?;
                let mut args = args.into_iter();
                let first = args
                    .next()
                    .ok_or_else(|| input.error("format! needs a literal format string"))?;
                if !matches!(&first, Expr::Lit(l) if matches!(l.lit, syn::Lit::Str(_))) {
                    return Err(input.error("format string must be a string literal"));
                }
                let mut out = vec![first];
                for arg in args {
                    if let Expr::Assign(a) = arg {
                        if !matches!(&*a.left, Expr::Path(p) if p.path.get_ident().is_some()) {
                            return Err(input.error("invalid named format argument"));
                        }
                        out.push(*a.right);
                    } else {
                        out.push(arg);
                    }
                }
                Ok(Inputs::from(out))
            })
            .parse2(mac.tokens.clone()),
            "matches" => (|input: ParseStream<'_>| {
                let value: Expr = input.parse()?;
                input.parse::<Token![,]>()?;
                let pat = Pat::parse_multi_with_leading_vert(input)?;
                let guard = if input.peek(Token![if]) {
                    input.parse::<Token![if]>()?;
                    let expr: Expr = input.parse()?;
                    Some(expr)
                } else {
                    None
                };
                if input.peek(Token![,]) {
                    input.parse::<Token![,]>()?;
                }
                let mut expressions = vec![value];
                expressions.extend(guard);
                Ok(Inputs {
                    expressions,
                    pattern: Some(pat),
                })
            })
            .parse2(mac.tokens.clone()),
            _ => {
                return Err(fail(
                    "only validated format!, matches! and vec! inputs are supported here",
                ));
            }
        };
        parsed.map_err(|e| fail(format!("invalid {name}! inputs: {e}")))
    }
}
pub(crate) fn fail(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new("E_UNSUPPORTED_MACRO", message)
}

/// Reject hidden matches instead of silently undercounting original selectors.
pub(crate) fn check_sites(hidden: &[Expr], injection: &Injection) -> Result<()> {
    struct Hidden<'a> {
        injection: &'a Injection,
        found: bool,
    }
    impl<'ast> Visit<'ast> for Hidden<'_> {
        fn visit_expr(&mut self, e: &'ast Expr) {
            let i = self.injection;
            if let Some(s) = &i.selector {
                self.found |= match i.kind {
                    Kind::ModifyConstant => sites::literal_selector(s)
                        .ok()
                        .is_some_and(|key| sites::constant_key(e).as_ref() == Some(&key)),
                    Kind::ModifyExpressionValue => syn::parse_str::<Expr>(s)
                        .ok()
                        .is_some_and(|expr| tokens(&expr) == tokens(e)),
                    _ => sites::parse_call_selector(s)
                        .ok()
                        .is_some_and(|expr| sites::call_matches(e, &expr)),
                };
            }
            if let Some(slice) = &i.slice {
                for anchor in [&slice.from, &slice.to].into_iter().flatten() {
                    self.found |= sites::parse_call_selector(&anchor.selector)
                        .ok()
                        .is_some_and(|expr| sites::call_matches(e, &expr));
                }
            }
            if let Some(v) = &i.variable {
                let name = v.name.strip_prefix("r#").unwrap_or(&v.name);
                self.found |= matches!(e, Expr::Path(p) if p.path.get_ident().is_some_and(|n| n.to_string().trim_start_matches("r#") == name));
                // Include implicit format captures and dynamic width/precision. False
                // positives in literal text are preferable to missing an access.
                if let Expr::Lit(l) = e
                    && let syn::Lit::Str(s) = &l.lit
                {
                    let text = s.value();
                    // The zero-padding flag can directly precede a width name
                    // (`0width$`). Unicode identifiers need NFC resolution,
                    // which this syntactic validator deliberately does not guess.
                    self.found |= (!name.is_ascii() && text.contains('{'))
                        || text
                            .split(|c: char| !c.is_alphanumeric() && c != '_')
                            .any(|word| {
                                word.trim_start_matches(|c: char| c.is_ascii_digit()) == name
                            });
                }
            }
            visit::visit_expr(self, e);
        }
    }
    let mut check = Hidden {
        injection,
        found: false,
    };
    for expr in hidden {
        check.visit_expr(expr);
    }
    if check.found {
        Err(Diagnostic::new(
            "E_UNSUPPORTED_MACRO_SITE",
            "selector or selected variable also occurs inside preserved macro inputs; move that code outside the macro",
        ))
    } else {
        Ok(())
    }
}

//! Textually scoped `macro_rules!` definitions, expanded only to validate hook sites.
//!
//! The planner never rewrites a macro invocation. To prove that an invocation hides no
//! control flow and no selector match, it substitutes the invocation's arguments into the
//! matching rule and checks the resulting expression like any other preserved macro input.
//! Only a deliberately small subset is understood: flat matchers made of literal tokens and
//! `ident`, `expr`, `tt`, `literal`, `ty` and `path` fragments, with a single-expression
//! transcriber. Anything else is rejected rather than guessed.
use crate::{Diagnostic, Result, standard_macros::fail};
use proc_macro2::{Delimiter, Group, TokenStream, TokenTree};
use quote::ToTokens;
use std::collections::BTreeMap;
use syn::{
    Expr, Lit, Path, Type,
    ext::IdentExt,
    parse::{ParseStream, Parser},
};

#[derive(Clone, Copy)]
enum Fragment {
    Ident,
    Expr,
    Tt,
    Literal,
    Ty,
    Path,
}
enum Element {
    Token(TokenTree),
    Fragment(String, Fragment),
}
type Bindings = BTreeMap<String, (Fragment, TokenStream)>;

fn unsupported(what: &str) -> Diagnostic {
    fail(format!(
        "local macro_rules! {what} is not supported; use flat matchers with ident, expr, tt, literal, ty or path fragments and a single-expression body"
    ))
}

fn is_punct(tree: Option<&TokenTree>, c: char) -> bool {
    matches!(tree, Some(TokenTree::Punct(p)) if p.as_char() == c)
}

/// Expand `input` with the first rule of `rules` that matches, as one expression.
pub(crate) fn expand(name: &str, rules: &TokenStream, input: &TokenStream) -> Result<Expr> {
    let mut trees = rules.clone().into_iter().peekable();
    while let Some(matcher) = trees.next() {
        let (TokenTree::Group(matcher), eq, gt, body) =
            (matcher, trees.next(), trees.next(), trees.next())
        else {
            return Err(unsupported("definition"));
        };
        let Some(TokenTree::Group(body)) = body else {
            return Err(unsupported("definition"));
        };
        if !is_punct(eq.as_ref(), '=') || !is_punct(gt.as_ref(), '>') {
            return Err(unsupported("definition"));
        }
        if is_punct(trees.peek(), ';') {
            trees.next();
        }
        let elements = elements(matcher.stream())?;
        let Some(bindings) = matches(&elements, input) else {
            continue;
        };
        let expansion = transcribe(body.stream(), &bindings)?;
        return syn::parse2::<Expr>(expansion).map_err(|e| {
            fail(format!(
                "expansion of local macro {name}! is not a single expression: {e}"
            ))
        });
    }
    Err(fail(format!(
        "no rule of local macro {name}! matches this invocation"
    )))
}

fn elements(matcher: TokenStream) -> Result<Vec<Element>> {
    let mut out = Vec::new();
    let mut trees = matcher.into_iter();
    while let Some(tree) = trees.next() {
        match tree {
            TokenTree::Punct(p) if p.as_char() == '$' => {
                let (Some(TokenTree::Ident(name)), colon, Some(TokenTree::Ident(kind))) =
                    (trees.next(), trees.next(), trees.next())
                else {
                    return Err(unsupported("repetition or malformed fragment"));
                };
                if !is_punct(colon.as_ref(), ':') {
                    return Err(unsupported("malformed fragment"));
                }
                let kind = match kind.to_string().as_str() {
                    "ident" => Fragment::Ident,
                    "expr" => Fragment::Expr,
                    "tt" => Fragment::Tt,
                    "literal" => Fragment::Literal,
                    "ty" => Fragment::Ty,
                    "path" => Fragment::Path,
                    _ => return Err(unsupported("fragment kind")),
                };
                out.push(Element::Fragment(name.to_string(), kind));
            }
            TokenTree::Group(_) => return Err(unsupported("nested matcher group")),
            tree => out.push(Element::Token(tree)),
        }
    }
    Ok(out)
}

fn matches(elements: &[Element], input: &TokenStream) -> Option<Bindings> {
    (|stream: ParseStream<'_>| {
        let mut bindings = Bindings::new();
        for element in elements {
            match element {
                Element::Token(expected) => {
                    let got: TokenTree = stream.parse()?;
                    if got.to_string() != expected.to_string() {
                        return Err(stream.error("token mismatch"));
                    }
                }
                Element::Fragment(name, kind) => {
                    let tokens = match kind {
                        Fragment::Ident => {
                            let ident = proc_macro2::Ident::parse_any(stream)?;
                            if ident == "_" {
                                return Err(stream.error("`_` is not an identifier"));
                            }
                            ident.into_token_stream()
                        }
                        Fragment::Expr => stream.parse::<Expr>()?.into_token_stream(),
                        Fragment::Tt => stream.parse::<TokenTree>()?.into_token_stream(),
                        Fragment::Literal => stream.parse::<Lit>()?.into_token_stream(),
                        Fragment::Ty => stream.parse::<Type>()?.into_token_stream(),
                        Fragment::Path => stream.parse::<Path>()?.into_token_stream(),
                    };
                    bindings.insert(name.clone(), (*kind, tokens));
                }
            }
        }
        if stream.is_empty() {
            Ok(bindings)
        } else {
            Err(stream.error("unexpected trailing tokens"))
        }
    })
    .parse2(input.clone())
    .ok()
}

fn transcribe(body: TokenStream, bindings: &Bindings) -> Result<TokenStream> {
    let mut out = TokenStream::new();
    let mut trees = body.into_iter();
    while let Some(tree) = trees.next() {
        match tree {
            TokenTree::Punct(p) if p.as_char() == '$' => match trees.next() {
                Some(TokenTree::Ident(name)) if name == "crate" => {
                    out.extend([TokenTree::Ident(proc_macro2::Ident::new(
                        "crate",
                        name.span(),
                    ))]);
                }
                Some(TokenTree::Ident(name)) => {
                    let Some((kind, tokens)) = bindings.get(&name.to_string()) else {
                        return Err(unsupported("transcriber variable without a binding"));
                    };
                    if matches!(kind, Fragment::Expr) {
                        // Keep operator precedence, as macro_rules does for expr fragments.
                        out.extend([TokenTree::Group(Group::new(
                            Delimiter::Parenthesis,
                            tokens.clone(),
                        ))]);
                    } else {
                        out.extend(tokens.clone());
                    }
                }
                _ => return Err(unsupported("repetition")),
            },
            TokenTree::Group(group) => {
                let mut inner =
                    Group::new(group.delimiter(), transcribe(group.stream(), bindings)?);
                inner.set_span(group.span());
                out.extend([TokenTree::Group(inner)]);
            }
            tree => out.extend([tree]),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn run(rules: TokenStream, input: TokenStream) -> Result<String> {
        expand("m", &rules, &input).map(|e| e.to_token_stream().to_string())
    }
    #[test]
    fn self_is_an_ident_argument_and_the_first_matching_rule_wins() {
        // The shape of Horizon's `world_items!(self)`.
        let rules = quote! {
            ($s:ident) => { match &$s.tables { Some(t) => &t.items, None => $s.res.items() } };
            ($s:ident, $n:literal) => { $n };
        };
        let out = run(rules.clone(), quote!(self)).unwrap();
        assert!(out.contains("& self . tables"), "{out}");
        assert!(out.contains("self . res . items ()"), "{out}");
        assert_eq!(run(rules, quote!(self, 4)).unwrap(), "4");
    }
    #[test]
    fn expr_arguments_keep_precedence_and_unsupported_shapes_fail() {
        let rules = quote! { ($a:expr, $b:expr) => { $a * $b }; };
        assert_eq!(run(rules, quote!(1 + 2, 3)).unwrap(), "(1 + 2) * (3)");
        for rules in [
            quote! { ($($a:expr),*) => { 1 }; },
            quote! { ($a:expr) => { $($a)* }; },
            quote! { ($a:expr) => { $b }; },
            quote! { (($a:expr)) => { $a }; },
        ] {
            let e = run(rules, quote!(1)).unwrap_err();
            assert_eq!(e.code, "E_UNSUPPORTED_MACRO");
        }
        let e = run(quote! { (a) => { 1 }; }, quote!(b)).unwrap_err();
        assert!(e.to_string().contains("no rule"), "{e}");
    }
}

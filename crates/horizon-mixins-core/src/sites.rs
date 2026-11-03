//! Original-AST expression coordinates shared by planning and transformation.
use crate::{Diagnostic, Result, index::tokens, source};
use quote::ToTokens;
use syn::{
    Expr,
    visit::{self, Visit},
};

pub fn contains_closure(expr: &Expr) -> bool {
    struct Find(bool);
    impl<'ast> Visit<'ast> for Find {
        fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {
            self.0 = true;
        }
    }
    let mut find = Find(false);
    find.visit_expr(expr);
    find.0
}
pub fn is_call(expr: &Expr) -> bool {
    matches!(expr, Expr::Call(_) | Expr::MethodCall(_))
}
/// Compact token boundaries, preserving literal spelling and embedded whitespace.
fn compact(stream: proc_macro2::TokenStream) -> String {
    stream
        .into_iter()
        .map(|token| match token {
            proc_macro2::TokenTree::Group(group) => {
                let (open, close) = match group.delimiter() {
                    proc_macro2::Delimiter::Parenthesis => ("(", ")"),
                    proc_macro2::Delimiter::Brace => ("{", "}"),
                    proc_macro2::Delimiter::Bracket => ("[", "]"),
                    proc_macro2::Delimiter::None => ("", ""),
                };
                format!("{open}{}{close}", compact(group.stream()))
            }
            other => other.to_string(),
        })
        .collect()
}
pub fn call_key(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Call(c) => Some(compact(c.func.to_token_stream())),
        Expr::MethodCall(c) => Some(format!(
            "{}.{}{}",
            compact(c.receiver.to_token_stream()),
            c.method,
            compact(c.turbofish.to_token_stream())
        )),
        _ => None,
    }
}
pub fn constant_key(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(l)
            if matches!(
                l.lit,
                syn::Lit::Int(_)
                    | syn::Lit::Float(_)
                    | syn::Lit::Bool(_)
                    | syn::Lit::Char(_)
                    | syn::Lit::Str(_)
            ) =>
        {
            Some(tokens(expr))
        }
        Expr::Unary(u)
            if matches!(u.op, syn::UnOp::Neg(_))
                && matches!(&*u.expr, Expr::Lit(l) if matches!(l.lit, syn::Lit::Int(_) | syn::Lit::Float(_))) =>
        {
            Some(tokens(expr))
        }
        _ => None,
    }
}
fn call_identity(expr: &Expr) -> Option<(bool, String, String)> {
    match expr {
        Expr::Call(c) => Some((false, tokens(&c.func), String::new())),
        Expr::MethodCall(c) => Some((
            true,
            tokens(&c.receiver),
            format!("{}{}", c.method, tokens(&c.turbofish)),
        )),
        _ => None,
    }
}
pub fn call_matches(expr: &Expr, selector: &Expr) -> bool {
    // Token serialization preserves identifier boundaries and literal spelling.
    // A cast such as `a as i32` must never alias an identifier named `aasi32`.
    let key = call_identity(expr);
    key.is_some() && key == call_identity(selector)
}
pub fn parse_call_selector(selector: &str) -> Result<Expr> {
    let parsed = syn::parse_str::<Expr>(&format!("{selector}()"));
    if parsed.as_ref().ok().and_then(call_key).as_deref() != Some(selector) {
        return Err(Diagnostic::new(
            "E_SELECTOR",
            "selector must be an exact callable path or receiver.method without whitespace outside literals",
        ));
    }
    parsed.map_err(|e| Diagnostic::new("E_SELECTOR", e.to_string()))
}
pub fn literal_selector(selector: &str) -> Result<String> {
    syn::parse_str::<Expr>(selector).ok().as_ref().and_then(constant_key)
        .ok_or_else(|| Diagnostic::new("E_SELECTOR", "constant selector must be a numeric, bool, char or string literal (optionally a negative number)"))
}
#[derive(Debug)]
pub struct Site {
    pub index: usize,
    pub expression: usize,
    pub expr: Expr,
    pub fingerprint: String,
    pub whole_expression: bool,
    pub const_context: bool,
    /// Inside a closure body: the closure may run later, elsewhere or never.
    pub in_closure: bool,
}
#[derive(Default)]
pub struct Sites {
    pub calls: Vec<Site>,
    pub constants: Vec<Site>,
    pub expressions: usize,
    pub values: Vec<Site>,
    next_is_root: bool,
    const_context: bool,
    closures: usize,
}
impl Sites {
    pub fn in_block(block: &syn::Block) -> Self {
        let mut sites = Self::default();
        sites.visit_block(block);
        sites
    }
}
impl<'ast> Visit<'ast> for Sites {
    fn visit_attribute(&mut self, _: &'ast syn::Attribute) {}
    fn visit_type(&mut self, _: &'ast syn::Type) {}
    fn visit_pat(&mut self, _: &'ast syn::Pat) {}
    fn visit_stmt(&mut self, stmt: &'ast syn::Stmt) {
        match stmt {
            syn::Stmt::Expr(expr, _) => {
                self.next_is_root = true;
                self.visit_expr(expr);
            }
            syn::Stmt::Local(local) => {
                if let Some(init) = &local.init {
                    self.next_is_root = plain_pattern(&local.pat);
                    self.visit_expr(&init.expr);
                    if let Some((_, expr)) = &init.diverge {
                        self.visit_expr(expr);
                    }
                }
            }
            _ => visit::visit_stmt(self, stmt),
        }
    }
    fn visit_expr_closure(&mut self, closure: &'ast syn::ExprClosure) {
        self.closures += 1;
        visit::visit_expr_closure(self, closure);
        self.closures -= 1;
    }
    fn visit_expr_repeat(&mut self, expr: &'ast syn::ExprRepeat) {
        self.visit_expr(&expr.expr);
        let old = std::mem::replace(&mut self.const_context, true);
        self.visit_expr(&expr.len);
        self.const_context = old;
    }
    fn visit_generic_argument(&mut self, arg: &'ast syn::GenericArgument) {
        let old = self.const_context;
        self.const_context |= matches!(
            arg,
            syn::GenericArgument::Const(_) | syn::GenericArgument::AssocConst(_)
        );
        visit::visit_generic_argument(self, arg);
        self.const_context = old;
    }
    fn visit_expr(&mut self, expr: &'ast Expr) {
        let root = std::mem::take(&mut self.next_is_root);
        let expression = self.expressions;
        self.expressions += 1;
        let site = |index| Site {
            index,
            expression,
            expr: expr.clone(),
            fingerprint: source::fingerprint(tokens(expr)),
            whole_expression: root,
            const_context: self.const_context,
            in_closure: self.closures > 0,
        };
        self.values.push(site(expression));
        if is_call(expr) {
            self.calls.push(site(self.calls.len()));
        }
        if constant_key(expr).is_some() {
            self.constants.push(site(self.constants.len()));
            return;
        }
        if matches!(expr, Expr::Paren(_)) {
            self.next_is_root = root;
        }
        visit::visit_expr(self, expr);
    }
}

/// Syntactic proof used only when a non-unit function has no implicit tail.
/// Unknown diverging calls are rejected rather than guessing their return type.
pub fn block_always_exits(block: &syn::Block) -> bool {
    block
        .stmts
        .iter()
        .any(|stmt| matches!(stmt, syn::Stmt::Expr(expr, _) if always_exits(expr)))
}
pub fn always_exits(expr: &Expr) -> bool {
    match expr {
        Expr::Return(_) => true,
        Expr::Block(b) => block_always_exits(&b.block),
        Expr::Paren(p) => always_exits(&p.expr),
        Expr::Group(g) => always_exits(&g.expr),
        Expr::If(i) => {
            always_exits(&i.cond)
                || (block_always_exits(&i.then_branch)
                    && i.else_branch.as_ref().is_some_and(|(_, e)| always_exits(e)))
        }
        Expr::Match(m) => {
            always_exits(&m.expr)
                || (!m.arms.is_empty() && m.arms.iter().all(|arm| always_exits(&arm.body)))
        }
        Expr::Loop(l) => {
            struct Breaks(bool);
            impl<'ast> Visit<'ast> for Breaks {
                fn visit_expr_break(&mut self, _: &'ast syn::ExprBreak) {
                    self.0 = true;
                }
            }
            let mut breaks = Breaks(false);
            breaks.visit_block(&l.body);
            !breaks.0
        }
        _ => false,
    }
}
pub fn is_unit(ty: &syn::Type) -> bool {
    matches!(ty, syn::Type::Tuple(t) if t.elems.is_empty())
}

/// Whole call under parentheses, with its pristine global expression coordinate.
pub fn root_call(expr: &Expr, expression: usize) -> Option<usize> {
    match expr {
        Expr::Paren(p) => root_call(&p.expr, expression + 1),
        _ if is_call(expr) => Some(expression),
        _ => None,
    }
}
pub fn plain_pattern(pat: &syn::Pat) -> bool {
    match pat {
        syn::Pat::Ident(p) => p.by_ref.is_none() && p.subpat.is_none(),
        syn::Pat::Type(p) => plain_pattern(&p.pat),
        _ => false,
    }
}
pub fn scalar(ty: &str) -> bool {
    [
        "bool", "char", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64",
        "i128", "isize", "f32", "f64",
    ]
    .contains(&ty)
}
pub fn validate_value_site(expr: &Expr, place: bool, ty: &str) -> Result<()> {
    let supported = !place
        && match expr {
            Expr::Call(_)
            | Expr::MethodCall(_)
            | Expr::Lit(_)
            | Expr::If(_)
            | Expr::Match(_)
            | Expr::Block(_)
            | Expr::Tuple(_)
            | Expr::Array(_)
            | Expr::Struct(_)
            | Expr::Cast(_) => true,
            Expr::Unary(e) => matches!(e.op, syn::UnOp::Neg(_) | syn::UnOp::Not(_)),
            Expr::Binary(e) => !matches!(
                e.op,
                syn::BinOp::AddAssign(_)
                    | syn::BinOp::SubAssign(_)
                    | syn::BinOp::MulAssign(_)
                    | syn::BinOp::DivAssign(_)
                    | syn::BinOp::RemAssign(_)
                    | syn::BinOp::BitXorAssign(_)
                    | syn::BinOp::BitAndAssign(_)
                    | syn::BinOp::BitOrAssign(_)
                    | syn::BinOp::ShlAssign(_)
                    | syn::BinOp::ShrAssign(_)
            ),
            Expr::Path(_) | Expr::Field(_) | Expr::Index(_) => scalar(ty),
            _ => false,
        };
    if supported {
        Ok(())
    } else {
        Err(Diagnostic::new(
            "E_UNSUPPORTED_SITE",
            "expression modifiers require a runtime value; place/reference/control-transfer uses and owned place reads are unsupported",
        ))
    }
}

//! Lexical identities on pristine syntax, with explicit barriers for unsupported patterns.
use crate::{
    Diagnostic, Result, helpers,
    index::{Function, tokens},
    manifest::{Access, Binding, Kind, Variable},
    sites, source,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use syn::{
    Expr, Pat, Type,
    visit::{self, Visit},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Local {
    pub id: usize,
    pub name: String,
    pub ordinal: usize,
    pub parameter: bool,
    pub mutable: bool,
    pub supported: bool,
    pub ty: Option<String>,
    pub fingerprint: String,
    /// Declared inside a closure: never visible where a hook runs (hooks refuse closures around
    /// their site), so it neither takes nor shifts the ordinals captures name.
    #[serde(skip)]
    pub in_closure: bool,
}
#[derive(Debug, Clone)]
pub struct Context {
    pub visible: BTreeMap<String, usize>,
    pub block: usize,
    pub depth: usize,
    pub constant: bool,
    pub place: bool,
}
#[derive(Debug, Clone)]
pub struct VariableSite {
    pub local: usize,
    pub expression: usize,
    pub at: Access,
    pub supported: bool,
    pub fingerprint: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub binding: String,
    pub local: Local,
    pub expression: String,
    pub ty: String,
}
#[derive(Debug, Clone)]
pub struct CaptureSpec {
    pub name: String,
    pub ordinal: usize,
    pub mode: CaptureMode,
}
#[derive(Debug, Clone, Copy)]
pub enum CaptureMode {
    Value,
    Ref,
    Mut,
}
pub fn parse_capture(value: &str) -> Result<Option<CaptureSpec>> {
    let (tail, mode) = if let Some(s) = value.strip_prefix("$local:") {
        (s, CaptureMode::Value)
    } else if let Some(s) = value.strip_prefix("$local_ref:") {
        (s, CaptureMode::Ref)
    } else if let Some(s) = value.strip_prefix("$local_mut:") {
        (s, CaptureMode::Mut)
    } else {
        return Ok(None);
    };
    let Some((name, ordinal)) = tail.rsplit_once(':') else {
        return Err(Diagnostic::new(
            "E_INVALID_CAPTURE",
            "capture requires NAME:ORDINAL",
        ));
    };
    syn::parse_str::<syn::Ident>(name).map_err(|_| {
        Diagnostic::new(
            "E_INVALID_CAPTURE",
            "capture name must be one Rust identifier",
        )
    })?;
    let ordinal = ordinal.parse().map_err(|_| {
        Diagnostic::new(
            "E_INVALID_CAPTURE",
            "capture ordinal must be a nonnegative integer",
        )
    })?;
    Ok(Some(CaptureSpec {
        name: name.into(),
        ordinal,
        mode,
    }))
}
#[derive(Default)]
pub struct Scopes {
    pub locals: Vec<Local>,
    pub contexts: Vec<Context>,
    pub accesses: Vec<VariableSite>,
    pub returns: Vec<usize>,
    pub after_assign: BTreeMap<usize, Context>,
    pub end: Option<Context>,
    frames: Vec<(usize, BTreeMap<String, usize>)>,
    next_block: usize,
    constant: bool,
    place: bool,
    ignore_reads: bool,
    /// How many closures the visitor is inside.
    closures: usize,
}
impl Scopes {
    pub fn analyze(f: &Function) -> Self {
        let mut scopes = Self::default();
        scopes.frames.push((0, BTreeMap::new()));
        for input in &f.signature.inputs {
            // A destructured parameter declares each name its pattern binds; their types
            // are not written anywhere, so captures of them need an explicit type.
            let declared: Vec<(String, Option<String>, bool)> = match input {
                syn::FnArg::Typed(arg) => match &*arg.pat {
                    Pat::Ident(p) => vec![(
                        p.ident.to_string(),
                        Some(tokens(&arg.ty)),
                        p.mutability.is_some(),
                    )],
                    pat => crate::index::pattern_bindings(pat)
                        .map(|names| {
                            names
                                .into_iter()
                                .map(|(name, mutable)| (name, None, mutable))
                                .collect()
                        })
                        .unwrap_or_default(),
                },
                syn::FnArg::Receiver(r) => {
                    vec![("self".into(), Some(tokens(&r.ty)), r.mutability.is_some())]
                }
            };
            for (name, ty, mutable) in declared {
                let id = scopes.locals.len();
                scopes.locals.push(Local {
                    id,
                    name: name.clone(),
                    ordinal: 0,
                    parameter: true,
                    mutable,
                    supported: true,
                    ty,
                    fingerprint: source::fingerprint(tokens(input)),
                    in_closure: false,
                });
                scopes.frames[0].1.insert(name, id);
            }
        }
        scopes.visit_block(&f.block);
        scopes
    }
    fn context(&self) -> Context {
        let mut visible = BTreeMap::new();
        for (_, frame) in &self.frames {
            visible.extend(frame.clone());
        }
        Context {
            visible,
            block: self.frames.last().map_or(0, |f| f.0),
            depth: self.frames.len() - 1,
            constant: self.constant,
            place: self.place,
        }
    }
    fn push(&mut self) {
        self.next_block += 1;
        self.frames.push((self.next_block, BTreeMap::new()));
    }
    fn add_pattern(&mut self, pat: &Pat, allow: bool, fingerprint: &str) -> Vec<usize> {
        struct Names(Vec<(String, bool)>);
        impl<'ast> Visit<'ast> for Names {
            fn visit_pat_ident(&mut self, p: &'ast syn::PatIdent) {
                self.0.push((p.ident.to_string(), p.mutability.is_some()));
                visit::visit_pat_ident(self, p);
            }
        }
        let (plain, ty) = match pat {
            Pat::Ident(p) => (p.by_ref.is_none() && p.subpat.is_none(), None),
            Pat::Type(t) => (
                matches!(&*t.pat,Pat::Ident(p) if p.by_ref.is_none()&&p.subpat.is_none()),
                Some(tokens(&t.ty)),
            ),
            _ => (false, None),
        };
        let mut names = Names(Vec::new());
        names.visit_pat(pat);
        names
            .0
            .into_iter()
            .map(|(name, mutable)| {
                let id = self.locals.len();
                let in_closure = self.closures > 0;
                let ordinal = self
                    .locals
                    .iter()
                    .filter(|l| !l.parameter && l.in_closure == in_closure && l.name == name)
                    .count();
                self.locals.push(Local {
                    id,
                    name,
                    ordinal,
                    parameter: false,
                    mutable,
                    supported: allow && plain,
                    ty: ty.clone(),
                    fingerprint: fingerprint.into(),
                    in_closure,
                });
                id
            })
            .collect()
    }
    fn expose(&mut self, ids: &[usize]) {
        for id in ids {
            self.frames
                .last_mut()
                .unwrap()
                .1
                .insert(self.locals[*id].name.clone(), *id);
        }
    }
    pub fn declaration(&self, name: &str, ordinal: usize, parameter: bool) -> Result<&Local> {
        self.locals
            .iter()
            .find(|l| {
                l.name == name && l.ordinal == ordinal && l.parameter == parameter && !l.in_closure
            })
            .ok_or_else(|| {
                Diagnostic::new(
                    "E_LOCAL_NOT_FOUND",
                    format!(
                        "no {} declaration {name}:{ordinal}",
                        if parameter { "parameter" } else { "local" }
                    ),
                )
            })
    }
    pub fn variable_sites(&self, variable: &Variable) -> Result<Vec<&VariableSite>> {
        let local = self.declaration(&variable.name, variable.ordinal, variable.parameter)?;
        if !local.supported {
            return Err(Diagnostic::new(
                "E_UNSUPPORTED_LOCAL",
                "destructuring/conditional pattern bindings are unsupported",
            ));
        }
        Ok(self
            .accesses
            .iter()
            .filter(|a| a.local == local.id && a.at == variable.at)
            .collect())
    }
    fn record_write(
        &mut self,
        left: &Expr,
        expression: usize,
        supported: bool,
        fingerprint: String,
    ) {
        if let Expr::Path(path) = left
            && path.qself.is_none()
            && path.path.segments.len() == 1
        {
            let name = path.path.segments[0].ident.to_string();
            if let Some(local) = self.context().visible.get(&name) {
                self.accesses.push(VariableSite {
                    local: *local,
                    expression,
                    at: Access::Store,
                    supported: supported && self.closures == 0,
                    fingerprint,
                });
            }
        } else if let Expr::Tuple(tuple) = left {
            for element in &tuple.elems {
                self.record_write(element, expression, false, fingerprint.clone());
            }
        }
    }
    fn visit_place(&mut self, expr: &Expr, ignore_reads: bool) {
        let old = self.place;
        let old_ignore = self.ignore_reads;
        self.place = true;
        self.ignore_reads = ignore_reads;
        self.visit_expr(expr);
        self.place = old;
        self.ignore_reads = old_ignore;
    }
}
impl<'ast> Visit<'ast> for Scopes {
    fn visit_attribute(&mut self, _: &'ast syn::Attribute) {}
    fn visit_type(&mut self, _: &'ast Type) {}
    fn visit_pat(&mut self, _: &'ast Pat) {}
    fn visit_block(&mut self, block: &'ast syn::Block) {
        self.push();
        for stmt in &block.stmts {
            self.visit_stmt(stmt);
        }
        if self.frames.len() == 2 {
            self.end = Some(self.context());
        }
        self.frames.pop();
    }
    fn visit_stmt(&mut self, stmt: &'ast syn::Stmt) {
        if let syn::Stmt::Local(local) = stmt {
            let fingerprint = source::fingerprint(tokens(local));
            let ids = self.add_pattern(&local.pat, true, &fingerprint);
            let mut call = None;
            if let Some(init) = &local.init {
                let expression = self.contexts.len();
                if sites::plain_pattern(&local.pat) && init.diverge.is_none() {
                    call = sites::root_call(&init.expr, expression);
                }
                self.visit_expr(&init.expr);
                for id in &ids {
                    self.accesses.push(VariableSite {
                        local: *id,
                        expression,
                        at: Access::Store,
                        supported: self.locals[*id].supported,
                        fingerprint: source::fingerprint(tokens(&init.expr)),
                    });
                }
                if let Some((_, expr)) = &init.diverge {
                    self.visit_expr(expr);
                }
            }
            self.expose(&ids);
            if let Some(call) = call {
                self.after_assign.insert(call, self.context());
            }
        } else {
            let expression = self.contexts.len();
            visit::visit_stmt(self, stmt);
            if let syn::Stmt::Expr(Expr::Assign(e), Some(_)) = stmt
                && matches!(&*e.left,Expr::Path(p) if p.path.segments.len()==1)
            {
                // Assignment visits its LHS before its RHS in all shared traversals.
                let left_count = expression_count(&e.left);
                if let Some(call) = sites::root_call(&e.right, expression + 1 + left_count) {
                    self.after_assign.insert(call, self.context());
                }
            }
        }
    }
    fn visit_expr_if(&mut self, e: &'ast syn::ExprIf) {
        self.push();
        self.visit_expr(&e.cond);
        self.visit_block(&e.then_branch);
        self.frames.pop();
        if let Some((_, other)) = &e.else_branch {
            self.visit_expr(other);
        }
    }
    fn visit_expr_while(&mut self, e: &'ast syn::ExprWhile) {
        self.push();
        self.visit_expr(&e.cond);
        self.visit_block(&e.body);
        self.frames.pop();
    }
    fn visit_expr_for_loop(&mut self, e: &'ast syn::ExprForLoop) {
        self.visit_expr(&e.expr);
        self.push();
        let ids = self.add_pattern(&e.pat, false, &source::fingerprint(tokens(&e.pat)));
        self.expose(&ids);
        self.visit_block(&e.body);
        self.frames.pop();
    }
    fn visit_expr_let(&mut self, e: &'ast syn::ExprLet) {
        self.visit_expr(&e.expr);
        let ids = self.add_pattern(&e.pat, false, &source::fingerprint(tokens(&e.pat)));
        self.expose(&ids);
    }
    fn visit_expr_match(&mut self, e: &'ast syn::ExprMatch) {
        self.visit_expr(&e.expr);
        for arm in &e.arms {
            self.push();
            let ids = self.add_pattern(&arm.pat, false, &source::fingerprint(tokens(&arm.pat)));
            self.expose(&ids);
            if let Some((_, guard)) = &arm.guard {
                self.visit_expr(guard);
            }
            self.visit_expr(&arm.body);
            self.frames.pop();
        }
    }
    fn visit_expr_assign(&mut self, e: &'ast syn::ExprAssign) {
        self.visit_place(&e.left, true);
        let expression = self.contexts.len();
        self.record_write(
            &e.left,
            expression,
            true,
            source::fingerprint(tokens(&e.right)),
        );
        self.visit_expr(&e.right);
    }
    fn visit_expr_binary(&mut self, e: &'ast syn::ExprBinary) {
        let compound = matches!(
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
        );
        if compound {
            self.visit_place(&e.left, true);
            self.record_write(
                &e.left,
                self.contexts.len(),
                false,
                source::fingerprint(tokens(&e.right)),
            );
            self.visit_expr(&e.right);
        } else {
            visit::visit_expr_binary(self, e);
        }
    }
    fn visit_expr_reference(&mut self, e: &'ast syn::ExprReference) {
        self.visit_place(&e.expr, false);
    }
    fn visit_expr_field(&mut self, e: &'ast syn::ExprField) {
        self.visit_place(&e.base, false);
    }
    fn visit_expr_index(&mut self, e: &'ast syn::ExprIndex) {
        self.visit_place(&e.expr, false);
        let old = self.place;
        self.place = false;
        self.visit_expr(&e.index);
        self.place = old;
    }
    fn visit_expr_method_call(&mut self, e: &'ast syn::ExprMethodCall) {
        self.visit_place(&e.receiver, false);
        if let Some(t) = &e.turbofish {
            self.visit_angle_bracketed_generic_arguments(t);
        }
        let old = self.place;
        self.place = false;
        for arg in &e.args {
            self.visit_expr(arg);
        }
        self.place = old;
    }
    fn visit_expr_repeat(&mut self, e: &'ast syn::ExprRepeat) {
        self.visit_expr(&e.expr);
        let old = self.constant;
        self.constant = true;
        self.visit_expr(&e.len);
        self.constant = old;
    }
    fn visit_generic_argument(&mut self, a: &'ast syn::GenericArgument) {
        let old = self.constant;
        self.constant |= matches!(
            a,
            syn::GenericArgument::Const(_) | syn::GenericArgument::AssocConst(_)
        );
        visit::visit_generic_argument(self, a);
        self.constant = old;
    }
    fn visit_expr_closure(&mut self, c: &'ast syn::ExprClosure) {
        self.closures += 1;
        visit::visit_expr_closure(self, c);
        self.closures -= 1;
    }
    fn visit_expr(&mut self, expr: &'ast Expr) {
        let expression = self.contexts.len();
        let context = self.context();
        self.contexts.push(context.clone());
        // A closure's own `return` leaves the closure: it is not an exit of the target.
        if matches!(expr, Expr::Return(_)) && self.closures == 0 {
            self.returns.push(expression);
        }
        if !self.ignore_reads
            && let Expr::Path(p) = expr
            && p.qself.is_none()
            && p.path.leading_colon.is_none()
            && p.path.segments.len() == 1
            && let Some(local) = context.visible.get(&p.path.segments[0].ident.to_string())
        {
            self.accesses.push(VariableSite {
                local: *local,
                expression,
                at: Access::Load,
                // Counted but never rewritten inside a closure (no hook runs there).
                supported: !self.place && !self.constant && self.closures == 0,
                fingerprint: source::fingerprint(tokens(expr)),
            });
        }
        if sites::constant_key(expr).is_some() {
            return;
        }
        visit::visit_expr(self, expr);
    }
}
pub fn resolve_bindings(
    f: &Function,
    scopes: &Scopes,
    bindings: &[Binding],
    contexts: &[&Context],
    kind: Kind,
) -> Result<Vec<Capture>> {
    let mut captures = Vec::new();
    for binding in bindings {
        if let Some(spec) = parse_capture(&binding.value)? {
            if matches!(kind, Kind::Head | Kind::Overwrite | Kind::WrapOperation) {
                return Err(Diagnostic::new(
                    "E_INVALID_CAPTURE",
                    "this injection has no original local context",
                ));
            }
            let local = scopes.declaration(&spec.name, spec.ordinal, false)?;
            if !local.supported {
                return Err(Diagnostic::new(
                    "E_UNSUPPORTED_LOCAL",
                    "capture requires a plain let binding; conditional/destructured bindings are unsupported",
                ));
            }
            if contexts.is_empty()
                || contexts
                    .iter()
                    .any(|c| c.visible.get(&local.name) != Some(&local.id))
            {
                return Err(Diagnostic::new(
                    "E_LOCAL_NOT_VISIBLE",
                    format!(
                        "{}:{} is out of scope, shadowed, or not initialized at this site/exit",
                        local.name, local.ordinal
                    ),
                ));
            }
            if matches!(spec.mode, CaptureMode::Mut) && !local.mutable {
                return Err(Diagnostic::new(
                    "E_INVALID_CAPTURE",
                    "mutable capture requires a mutable local binding",
                ));
            }
            let name: syn::Ident = syn::parse_str(&local.name).unwrap();
            let expression = match spec.mode {
                CaptureMode::Value => local.name.clone(),
                CaptureMode::Ref => format!("&{name}"),
                CaptureMode::Mut => format!("&mut {name}"),
            };
            if let Some(ty) = &local.ty {
                let ty: Type = syn::parse_str(ty)
                    .map_err(|e| Diagnostic::new("E_CAPTURE_TYPE", e.to_string()))?;
                let expected: Type = match spec.mode {
                    CaptureMode::Value => ty,
                    CaptureMode::Ref => syn::parse_quote!(&#ty),
                    CaptureMode::Mut => syn::parse_quote!(&mut #ty),
                };
                let declared: Type = syn::parse_str(&binding.ty)
                    .map_err(|e| Diagnostic::new("E_CAPTURE_TYPE", e.to_string()))?;
                if helpers::normalize_type(&expected) != helpers::normalize_type(&declared) {
                    return Err(Diagnostic::new(
                        "E_CAPTURE_TYPE",
                        format!("capture {} must use type {}", local.name, tokens(&expected)),
                    ));
                }
            }
            captures.push(Capture {
                binding: binding.name.clone(),
                local: local.clone(),
                expression,
                ty: binding.ty.clone(),
            });
        } else if !binding.value.starts_with('$') {
            struct Root(Option<String>);
            impl<'ast> Visit<'ast> for Root {
                fn visit_expr_path(&mut self, p: &'ast syn::ExprPath) {
                    if p.path.segments.len() == 1 {
                        self.0 = Some(p.path.segments[0].ident.to_string());
                    }
                }
            }
            let value: Expr = syn::parse_str(&binding.value)
                .map_err(|e| Diagnostic::new("E_INVALID_BINDING", e.to_string()))?;
            let mut root = Root(None);
            root.visit_expr(&value);
            if let Some(name) = root.0
                && let Some(parameter) =
                    scopes.locals.iter().find(|l| l.parameter && l.name == name)
                && contexts
                    .iter()
                    .any(|c| c.visible.get(&name) != Some(&parameter.id))
            {
                return Err(Diagnostic::new(
                    "E_INVALID_BINDING",
                    format!("target parameter '{name}' is shadowed at this site/exit"),
                ));
            }
        }
    }
    let _ = f;
    Ok(captures)
}

fn expression_count(expr: &Expr) -> usize {
    let block: syn::Block = syn::parse_quote!({ #expr });
    sites::Sites::in_block(&block).expressions
}

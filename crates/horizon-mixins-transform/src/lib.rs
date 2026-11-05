use horizon_mixins_core::{
    Diagnostic, Plan, PlannedInjection, Result, bridges,
    index::{Function, tokens},
    manifest::Kind,
    plan, sites, source,
};
use quote::{format_ident, quote};
use std::{collections::BTreeMap, path::Path};
use syn::{
    Block, Expr, Item, Signature, Type,
    visit_mut::{self, VisitMut},
};

/// Rewrite pristine expression coordinates once. Generated code is never matched.
pub fn transform(root: &Path, plan: &Plan) -> Result<BTreeMap<String, Vec<u8>>> {
    if plan.schema != plan::SCHEMA || plan.transformer != env!("CARGO_PKG_VERSION") {
        return Err(Diagnostic::new(
            "E_PLAN_VERSION",
            "unsupported plan schema/transformer",
        ));
    }
    plan.validate_structure()?;
    if plan.rustc != plan::rustc(root)? {
        return Err(Diagnostic::new(
            "E_TOOLCHAIN",
            "rustc differs from the planned toolchain",
        ));
    }
    let mut files = source::files(root)?;
    let fingerprints: BTreeMap<_, _> = files
        .iter()
        .map(|(name, bytes)| (name.clone(), source::fingerprint(bytes)))
        .collect();
    if fingerprints != plan.inputs {
        return Err(Diagnostic::new(
            "E_STALE_SOURCE",
            "workspace no longer matches planned inputs; regenerate plan",
        ));
    }
    let mut groups: BTreeMap<(&str, &[usize]), Vec<&PlannedInjection>> = BTreeMap::new();
    for patch in &plan.injections {
        groups
            .entry((&patch.file, &patch.node))
            .or_default()
            .push(patch);
    }
    for s in &plan.states {
        for c in &s.constructors {
            groups.entry((&c.file, &c.node)).or_default();
        }
    }
    let mut trees = BTreeMap::new();
    for (file, _) in groups.keys() {
        if !trees.contains_key(*file) {
            let bytes = files
                .get(*file)
                .ok_or_else(|| Diagnostic::new("E_STALE_SOURCE", "planned file is absent"))?;
            let text = std::str::from_utf8(bytes)
                .map_err(|e| Diagnostic::new("E_RUST_SYNTAX", e.to_string()))?;
            trees.insert(
                file.to_string(),
                syn::parse_file(text)
                    .map_err(|e| Diagnostic::new("E_RUST_SYNTAX", e.to_string()).file(file))?,
            );
        }
    }
    for bridge in &plan.bridges {
        if !trees.contains_key(&bridge.file) {
            let bytes = files
                .get(&bridge.file)
                .ok_or_else(|| Diagnostic::new("E_STALE_SOURCE", "bridge file is absent"))?;
            let text = std::str::from_utf8(bytes)
                .map_err(|e| Diagnostic::new("E_RUST_SYNTAX", e.to_string()))?;
            trees.insert(bridge.file.clone(), syn::parse_file(text).map_err(syntax)?);
        }
        let original = bridges::item(&trees[&bridge.file].items, &bridge.node)?;
        if source::fingerprint(tokens(original)) != bridge.fingerprint {
            return Err(
                Diagnostic::new("E_STALE_SOURCE", "bridge source fingerprint mismatch")
                    .file(&bridge.file),
            );
        }
    }
    for s in &plan.states {
        if !trees.contains_key(&s.file) {
            let bytes = files
                .get(&s.file)
                .ok_or_else(|| Diagnostic::new("E_PLAN_SITE", "state file absent"))?;
            trees.insert(
                s.file.clone(),
                syn::parse_file(
                    std::str::from_utf8(bytes)
                        .map_err(|e| Diagnostic::new("E_RUST_SYNTAX", e.to_string()))?,
                )
                .map_err(syntax)?,
            );
        }
        if source::fingerprint(tokens(bridges::item(&trees[&s.file].items, &s.node)?))
            != s.fingerprint
        {
            return Err(Diagnostic::new(
                "E_STALE_SOURCE",
                "state owner fingerprint mismatch",
            ));
        }
    }
    for module in &plan.modules {
        if !trees.contains_key(&module.file) {
            let bytes = files
                .get(&module.file)
                .ok_or_else(|| Diagnostic::new("E_STALE_SOURCE", "module destination absent"))?;
            let text = std::str::from_utf8(bytes)
                .map_err(|e| Diagnostic::new("E_RUST_SYNTAX", e.to_string()))?;
            trees.insert(module.file.clone(), syn::parse_file(text).map_err(syntax)?);
        }
        let tree = &trees[&module.file];
        if source::fingerprint(tokens(tree)) != module.fingerprint {
            return Err(Diagnostic::new(
                "E_STALE_SOURCE",
                "module destination fingerprint mismatch",
            ));
        }
        // Its glob imports were followed when it was planned, on this same source (the
        // fingerprints above): only its own names are checked again.
        horizon_mixins_core::modules::validate_destination_with(tree, &module.name, &mut |_| {
            false
        })?;
    }
    for ((file, node), mut patches) in groups {
        // Execution order is global within a function; site grouping happens below.
        patches.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then(a.mod_id.cmp(&b.mod_id))
                .then(a.injection_id.cmp(&b.injection_id))
        });
        let tree = trees.get_mut(file).unwrap();
        let (signature, block) = locate(&mut tree.items, node)?;
        let fingerprint = source::fingerprint(format!("{}{}", tokens(signature), tokens(block)));
        for patch in &patches {
            if patch.function_fingerprint != fingerprint {
                return Err(
                    Diagnostic::new("E_STALE_SOURCE", "function fingerprint mismatch").file(file),
                );
            }
            let f = Function {
                id: patch.target.clone(),
                module: String::new(),
                file: file.into(),
                node: node.to_vec(),
                signature: signature.clone(),
                block: block.clone(),
                unsupported_cfg: false,
                unsupported_macro: false,
                generic_impl: false,
                self_type: None,
            };
            plan::validate_helper(
                &f,
                patch.kind,
                patch.cancellable,
                &patch.helper_source,
                &patch.bindings,
                &patch.contract,
            )?;
        }
        if patches.iter().any(|p| p.kind == Kind::Overwrite) {
            if patches.len() != 1 {
                return Err(Diagnostic::new(
                    "E_OVERWRITE_CONFLICT",
                    "OVERWRITE must be exclusive",
                ));
            }
            let expr = helper_call(patches[0])?;
            *block = syn::parse2(quote!({ #expr })).map_err(syntax)?;
            continue;
        }
        let mut calls: BTreeMap<usize, Vec<&PlannedInjection>> = BTreeMap::new();
        let mut constants: BTreeMap<usize, Vec<&PlannedInjection>> = BTreeMap::new();
        for p in &patches {
            if let Some(site) = p.call_site {
                calls.entry(site).or_default().push(*p);
            }
            if let Some(site) = p.constant_site {
                constants.entry(site).or_default().push(*p);
            }
        }
        let mut variables: BTreeMap<usize, Vec<&PlannedInjection>> = BTreeMap::new();
        for p in &patches {
            if let Some(site) = p.variable_site {
                variables.entry(site).or_default().push(*p);
            }
        }
        for patches in variables.values_mut() {
            patches.sort_by_key(|p| {
                p.variable
                    .as_ref()
                    .is_some_and(|v| v.at == horizon_mixins_core::manifest::Access::Store)
            });
        }
        let mut expressions: BTreeMap<usize, Vec<&PlannedInjection>> = BTreeMap::new();
        for p in &patches {
            if p.kind == Kind::ModifyExpressionValue {
                expressions
                    .entry(p.expression_site.unwrap())
                    .or_default()
                    .push(*p);
            }
        }
        let mut states: BTreeMap<
            usize,
            Vec<(
                &horizon_mixins_core::state::PlannedState,
                &horizon_mixins_core::state::ConstructorSite,
            )>,
        > = BTreeMap::new();
        for s in &plan.states {
            for c in &s.constructors {
                if c.file == file && c.node == node {
                    if c.function_fingerprint != fingerprint {
                        return Err(Diagnostic::new(
                            "E_STALE_SOURCE",
                            "state constructor fingerprint mismatch",
                        ));
                    }
                    states.entry(c.expression).or_default().push((s, c));
                }
            }
        }
        let expected =
            calls.len() + constants.len() + variables.len() + expressions.len() + states.len();
        let mut rewrite = Rewrite {
            calls,
            constants,
            variables,
            expressions,
            states,
            target_result: horizon_mixins_core::helpers::return_type(signature),
            after_stack: Vec::new(),
            expression_cursor: 0,
            call_cursor: 0,
            constant_cursor: 0,
            matched: 0,
            error: None,
            next_discards_result: false,
        };
        if expected > 0 {
            rewrite.visit_block_mut(block);
        }
        if let Some(e) = rewrite.error {
            return Err(e.file(file));
        }
        if rewrite.matched != expected {
            return Err(
                Diagnostic::new("E_STALE_SOURCE", "planned expression site disappeared").file(file),
            );
        }
        let returns: Vec<_> = patches
            .iter()
            .filter(|p| p.kind == Kind::Return)
            .copied()
            .collect();
        if !returns.is_empty() {
            let mut rewrite = ReturnRewrite {
                patches: &returns,
                error: None,
            };
            rewrite.visit_block_mut(block);
            if let Some(e) = rewrite.error {
                return Err(e.file(file));
            }
            match block.stmts.last_mut() {
                Some(syn::Stmt::Expr(expr, None)) if !sites::always_exits(expr) => {
                    *expr = value_chain(expr.clone(), &returns)?
                }
                Some(syn::Stmt::Expr(expr, _)) if sites::always_exits(expr) => {}
                _ if sites::is_unit(&horizon_mixins_core::helpers::return_type(signature)) => {
                    let expr = value_chain(syn::parse_quote!(()), &returns)?;
                    block.stmts.push(syn::Stmt::Expr(expr, None));
                }
                _ => {} // Planning proved that every path exits explicitly.
            }
        }
        let mut head = Vec::new();
        for patch in patches.iter().filter(|p| p.kind == Kind::Head) {
            let expr = helper_call(patch)?;
            let statement = if patch.cancellable {
                let value: Expr = syn::parse_quote!(__horizon_mixins_return);
                let result = value_chain(value, &returns)?;
                syn::parse2(
                    quote!(if let ::core::ops::ControlFlow::Break(__horizon_mixins_return) = #expr { return #result; }),
                )
            } else {
                syn::parse2(quote!(#expr;))
            };
            head.push(statement.map_err(syntax)?);
        }
        head.append(&mut block.stmts);
        block.stmts = head;
    }
    for s in &plan.states {
        let tree = trees.get_mut(&s.file).unwrap();
        let parent = parent_items(&mut tree.items, &s.node[..s.node.len() - 1])?;
        let Some(Item::Struct(owner)) = parent.get_mut(*s.node.last().unwrap()) else {
            return Err(Diagnostic::new("E_PLAN_SITE", "state owner is not struct"));
        };
        let syn::Fields::Named(fields) = &mut owner.fields else {
            return Err(Diagnostic::new(
                "E_PLAN_SITE",
                "state owner has no named fields",
            ));
        };
        fields.named.push(s.field()?);
        parent.extend(s.items()?);
    }
    for bridge in &plan.bridges {
        let generated: Item = syn::parse_str(&bridge.generated).map_err(syntax)?;
        if !matches!(generated, Item::Impl(_)) {
            return Err(Diagnostic::new(
                "E_PLAN_SITE",
                "bridge must contain one impl",
            ));
        }
        let tree = trees.get_mut(&bridge.file).unwrap();
        parent_items(&mut tree.items, &bridge.node[..bridge.node.len() - 1])?.push(generated);
    }
    for module in &plan.modules {
        let name: syn::Ident = syn::parse_str(&module.name).map_err(syntax)?;
        let visibility = match module.visibility {
            horizon_mixins_core::manifest::ModuleVisibility::Crate => quote!(pub(crate)),
            horizon_mixins_core::manifest::ModuleVisibility::Public => quote!(pub),
        };
        let payload = horizon_mixins_core::modules::validate_payload(&module.source)?;
        let items = payload.items;
        trees
            .get_mut(&module.file)
            .unwrap()
            .items
            .push(syn::parse2(quote!(#visibility mod #name { #(#items)* })).map_err(syntax)?);
    }
    for (file, tree) in trees {
        files.insert(file, prettyplease::unparse(&tree).into_bytes());
    }
    Ok(files)
}
fn syntax(e: syn::Error) -> Diagnostic {
    Diagnostic::new("E_TRANSFORM", e.to_string())
}
fn parsed(text: &str) -> Result<Expr> {
    syn::parse_str(text).map_err(syntax)
}
fn helper_call(patch: &PlannedInjection) -> Result<Expr> {
    if !patch.captures.is_empty() {
        return invoke(patch, &BTreeMap::new());
    }
    let helper = plan::parse_helper(&patch.helper_source)?;
    let name = &helper.sig.ident;
    let values: Vec<Expr> = patch
        .bindings
        .iter()
        .map(|b| binding_value(patch, b, &BTreeMap::new()))
        .collect::<Result<_>>()?;
    syn::parse2(quote!({ #helper #name(#(#values),*) })).map_err(syntax)
}
/// Only the function item is scoped by a block. Original value expressions remain
/// call arguments in their existing temporary scope, including when nested.
fn invoke(patch: &PlannedInjection, specials: &BTreeMap<String, Expr>) -> Result<Expr> {
    let helper = plan::parse_helper(&patch.helper_source)?;
    let name = &helper.sig.ident;
    let values: Vec<Expr> = patch
        .bindings
        .iter()
        .map(|b| binding_value(patch, b, specials))
        .collect::<Result<_>>()?;
    syn::parse2(quote!(({ #helper #name })(#(#values),*))).map_err(syntax)
}
fn binding_value(
    patch: &PlannedInjection,
    binding: &horizon_mixins_core::manifest::Binding,
    specials: &BTreeMap<String, Expr>,
) -> Result<Expr> {
    if let Some(value) = specials.get(&binding.value) {
        return Ok(value.clone());
    }
    if let Some(capture) = patch.captures.iter().find(|c| c.binding == binding.name) {
        return parsed(&capture.expression);
    }
    if binding.value.starts_with('$') {
        return Err(Diagnostic::new(
            "E_PLAN_SITE",
            format!("unresolved special binding {}", binding.value),
        ));
    }
    parsed(&binding.value)
}

fn value_chain(mut expr: Expr, patches: &[&PlannedInjection]) -> Result<Expr> {
    for p in patches {
        expr = invoke(p, &BTreeMap::from([("$value".into(), expr)]))?;
    }
    Ok(expr)
}
fn control_statement(patch: &PlannedInjection) -> Result<syn::Stmt> {
    let call = helper_call(patch)?;
    if patch.cancellable {
        syn::parse2(quote!(if let ::core::ops::ControlFlow::Break(__horizon_mixins_cancel) = #call { return __horizon_mixins_cancel; })).map_err(syntax)
    } else {
        syn::parse2(quote!(#call;)).map_err(syntax)
    }
}
fn after_adapter(value: Expr, patch: &PlannedInjection, result: &Type, flow: bool) -> Result<Expr> {
    let ty: Type =
        syn::parse_str(
            patch.contract.result.as_deref().ok_or_else(|| {
                Diagnostic::new("E_PLAN_SITE", "missing AFTER_CALL result contract")
            })?,
        )
        .map_err(syntax)?;
    let mut params = Vec::new();
    let mut args = vec![value];
    let mut helper_args: Vec<Expr> = Vec::new();
    let value_name = format_ident!("__horizon_mixins_after_value");
    for (i, binding) in patch.bindings.iter().enumerate() {
        if binding.value == "$result_ref" {
            helper_args.push(syn::parse_quote!(&#value_name));
        } else if binding.value == "$result_mut" {
            helper_args.push(syn::parse_quote!(&mut #value_name));
        } else {
            let name = format_ident!("__horizon_mixins_context_{i}");
            let ty: Type = syn::parse_str(&binding.ty).map_err(syntax)?;
            params.push(quote!(#name:#ty));
            args.push(binding_value(patch, binding, &BTreeMap::new())?);
            helper_args.push(syn::parse_quote!(#name));
        }
    }
    let helper = plan::parse_helper(&patch.helper_source)?;
    let name = &helper.sig.ident;
    let call = quote!(#name(#(#helper_args),*));
    let output: Type = if flow {
        syn::parse_quote!(::core::ops::ControlFlow<#result,#ty>)
    } else {
        ty.clone()
    };
    let body = if patch.cancellable {
        quote!(if let ::core::ops::ControlFlow::Break(value)=#call {return ::core::ops::ControlFlow::Break(value);})
    } else {
        quote!(#call;)
    };
    let tail = if flow {
        quote!(::core::ops::ControlFlow::Continue(#value_name))
    } else {
        quote!(#value_name)
    };
    let mutable = patch
        .bindings
        .iter()
        .any(|b| b.value == "$result_mut")
        .then(|| quote!(mut));
    syn::parse2(quote!(({fn __horizon_mixins_after(#mutable #value_name:#ty,#(#params),*)->#output{#helper #body #tail} __horizon_mixins_after})(#(#args),*))).map_err(syntax)
}
fn after_chain(
    value: Expr,
    patches: &[&PlannedInjection],
    target: &Type,
    discarded: bool,
) -> Result<Expr> {
    if !patches.iter().any(|p| p.cancellable) {
        let mut value = value;
        for patch in patches {
            value = after_adapter(value, patch, target, false)?;
        }
        return Ok(value);
    }
    fn next(
        value: Expr,
        patches: &[&PlannedInjection],
        target: &Type,
        discarded: bool,
    ) -> Result<Expr> {
        if patches.is_empty() {
            return if discarded {
                syn::parse2(quote!({let _ = #value;})).map_err(syntax)
            } else {
                Ok(value)
            };
        }
        let call = after_adapter(value, patches[0], target, true)?;
        let value: Expr = syn::parse_quote!(__horizon_mixins_after_next);
        let tail = next(value, &patches[1..], target, discarded)?;
        syn::parse2(quote!(match #call {::core::ops::ControlFlow::Break(__horizon_mixins_cancel)=>return __horizon_mixins_cancel,::core::ops::ControlFlow::Continue(__horizon_mixins_after_next)=>#tail})).map_err(syntax)
    }
    next(value, patches, target, discarded)
}
fn wrap_operation(call: &Expr, wrappers: &[&PlannedInjection]) -> Result<Expr> {
    let operation = wrappers[0].contract.operation.as_ref().unwrap();
    let names: Vec<_> = (0..operation.inputs.len())
        .map(|i| format_ident!("__horizon_mixins_arg{i}"))
        .collect();
    let types: Vec<Type> = operation
        .inputs
        .iter()
        .map(|s| syn::parse_str(s).map_err(syntax))
        .collect::<Result<_>>()?;
    let inputs: Vec<Expr> = names.iter().map(|name| syn::parse_quote!(#name)).collect();
    let actual_count = match call {
        Expr::Call(c) => c.args.len(),
        Expr::MethodCall(c) => c.args.len() + 1,
        _ => return Err(Diagnostic::new("E_PLAN_SITE", "wrapper site is not a call")),
    };
    if actual_count != inputs.len() || operation.method != matches!(call, Expr::MethodCall(_)) {
        return Err(Diagnostic::new(
            "E_PLAN_SITE",
            "operation input shape differs from its original call",
        ));
    }

    let mut original = call.clone();
    let mut arguments: Vec<Expr>;
    match &mut original {
        Expr::Call(c) => {
            arguments = c.args.iter().cloned().collect();
            c.args = inputs.iter().cloned().collect();
        }
        Expr::MethodCall(c) => {
            let receiver = &c.receiver;
            let Type::Reference(reference) = &types[0] else {
                return Err(Diagnostic::new(
                    "E_OPERATION_SIGNATURE",
                    "method wrapper needs a reference receiver",
                ));
            };
            let receiver_arg = if reference.mutability.is_some() {
                syn::parse_quote!(&mut *(#receiver))
            } else {
                syn::parse_quote!(&*(#receiver))
            };
            arguments = vec![receiver_arg];
            arguments.extend(c.args.iter().cloned());
            *c.receiver = inputs[0].clone();
            c.args = inputs[1..].iter().cloned().collect();
        }
        _ => return Err(Diagnostic::new("E_PLAN_SITE", "wrapper site is not a call")),
    }
    let mut continuation: Expr =
        syn::parse2(quote!(|#(#names: #types),*| #original)).map_err(syntax)?;
    for p in wrappers.iter().rev() {
        let mut specials: BTreeMap<_, _> = inputs
            .iter()
            .enumerate()
            .map(|(i, value)| {
                let key = if operation.method && i == 0 {
                    "$receiver".into()
                } else {
                    format!("$arg{}", i - usize::from(operation.method))
                };
                (key, value.clone())
            })
            .collect();
        specials.insert("$operation".into(), continuation);
        let body = invoke(p, &specials)?;
        continuation = syn::parse2(quote!(|#(#names: #types),*| #body)).map_err(syntax)?;
    }
    syn::parse2(quote!((#continuation)(#(#arguments),*))).map_err(syntax)
}
fn bundle_arguments(
    call: &Expr,
    bundles: &[&PlannedInjection],
    wrappers: &[&PlannedInjection],
) -> Result<Expr> {
    let bundle = bundles[0].contract.bundle.as_ref().unwrap();
    let types: Vec<Type> = bundle
        .inputs
        .iter()
        .map(|s| syn::parse_str(s).map_err(syntax))
        .collect::<Result<_>>()?;
    let result: Type = syn::parse_str(&bundle.output).map_err(syntax)?;
    let names: Vec<_> = (0..types.len())
        .map(|i| format_ident!("__horizon_mixins_bundle_arg{i}"))
        .collect();
    let mut forward = call.clone();
    let args: Vec<Expr> = match &mut forward {
        Expr::Call(c) => {
            let args = c.args.iter().cloned().collect();
            c.args = names
                .iter()
                .map(|n| -> Expr { syn::parse_quote!(#n) })
                .collect();
            args
        }
        Expr::MethodCall(c) => {
            let args = c.args.iter().cloned().collect();
            c.args = names
                .iter()
                .map(|n| -> Expr { syn::parse_quote!(#n) })
                .collect();
            *c.receiver = syn::parse_quote!(__horizon_mixins_bundle_receiver);
            args
        }
        _ => return Err(Diagnostic::new("E_PLAN_SITE", "bundle site is not a call")),
    };
    if types.len() != args.len() || bundle.receiver.is_some() != matches!(call, Expr::MethodCall(_))
    {
        return Err(Diagnostic::new(
            "E_PLAN_SITE",
            "argument bundle shape differs from original call",
        ));
    }
    let mut tuple: Expr = syn::parse2(quote!((#(#args,)*))).map_err(syntax)?;
    for p in bundles {
        tuple = invoke(p, &BTreeMap::from([("$args".into(), tuple)]))?;
    }
    let body = if wrappers.is_empty() {
        forward
    } else {
        wrap_operation(&forward, wrappers)?
    };
    if let Some(receiver) = &bundle.receiver {
        let ty: Type = syn::parse_str(receiver).map_err(syntax)?;
        let Type::Reference(reference) = &ty else {
            return Err(Diagnostic::new(
                "E_PLAN_SITE",
                "bundle receiver must be a reference",
            ));
        };
        let Expr::MethodCall(original) = call else {
            return Err(Diagnostic::new("E_PLAN_SITE", "bundle method disappeared"));
        };
        let receiver = &original.receiver;
        let argument: Expr = if reference.mutability.is_some() {
            syn::parse_quote!(&mut *(#receiver))
        } else {
            syn::parse_quote!(&*(#receiver))
        };
        syn::parse2(quote!((|__horizon_mixins_bundle_receiver:#ty,(#(#names,)*):(#(#types,)*)|->#result{#body})(#argument,#tuple))).map_err(syntax)
    } else {
        syn::parse2(quote!((|(#(#names,)*):(#(#types,)*)|->#result{#body})(#tuple))).map_err(syntax)
    }
}

struct Rewrite<'a> {
    calls: BTreeMap<usize, Vec<&'a PlannedInjection>>,
    constants: BTreeMap<usize, Vec<&'a PlannedInjection>>,
    variables: BTreeMap<usize, Vec<&'a PlannedInjection>>,
    expressions: BTreeMap<usize, Vec<&'a PlannedInjection>>,
    states: BTreeMap<
        usize,
        Vec<(
            &'a horizon_mixins_core::state::PlannedState,
            &'a horizon_mixins_core::state::ConstructorSite,
        )>,
    >,
    target_result: Type,
    after_stack: Vec<Vec<syn::Stmt>>,
    expression_cursor: usize,
    call_cursor: usize,
    constant_cursor: usize,
    matched: usize,
    error: Option<Diagnostic>,
    next_discards_result: bool,
}
impl VisitMut for Rewrite<'_> {
    fn visit_attribute_mut(&mut self, _: &mut syn::Attribute) {}
    fn visit_type_mut(&mut self, _: &mut Type) {}
    fn visit_pat_mut(&mut self, _: &mut syn::Pat) {}
    fn visit_block_mut(&mut self, block: &mut Block) {
        let mut output = Vec::new();
        for mut stmt in std::mem::take(&mut block.stmts) {
            self.after_stack.push(Vec::new());
            self.visit_stmt_mut(&mut stmt);
            output.push(stmt);
            output.extend(self.after_stack.pop().unwrap());
        }
        block.stmts = output;
    }
    fn visit_stmt_mut(&mut self, stmt: &mut syn::Stmt) {
        self.next_discards_result = matches!(stmt, syn::Stmt::Expr(_, Some(_)));
        visit_mut::visit_stmt_mut(self, stmt);
    }
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        let discarded = std::mem::take(&mut self.next_discards_result);
        let expression = self.expression_cursor;
        self.expression_cursor += 1;
        let expression_fingerprint = source::fingerprint(tokens(expr));
        if sites::constant_key(expr).is_some() {
            let site = self.constant_cursor;
            self.constant_cursor += 1;
            if let Some(patches) = self.constants.get(&site) {
                if patches.iter().any(|p| {
                    p.site_fingerprint.as_deref() != Some(&source::fingerprint(tokens(expr)))
                }) {
                    self.error = Some(Diagnostic::new(
                        "E_STALE_SOURCE",
                        "constant site fingerprint mismatch",
                    ));
                    return;
                }
                match value_chain(expr.clone(), patches) {
                    Ok(new) => {
                        *expr = new;
                        self.matched += 1;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            self.modify_values(expr, expression, &expression_fingerprint);
            return;
        }
        let site = if sites::is_call(expr) {
            let site = self.call_cursor;
            self.call_cursor += 1;
            Some(site)
        } else {
            None
        };
        let original = source::fingerprint(tokens(expr));
        if matches!(expr, Expr::Paren(_)) {
            self.next_discards_result = discarded;
        }
        visit_mut::visit_expr_mut(self, expr);
        if let Some(patches) = site.and_then(|site| self.calls.get(&site)) {
            if patches
                .iter()
                .any(|p| p.site_fingerprint.as_deref() != Some(&original))
            {
                self.error = Some(Diagnostic::new(
                    "E_STALE_SOURCE",
                    "call site fingerprint mismatch",
                ));
                return;
            }
            let result = (|| {
                let mut new = expr.clone();
                let args = match &mut new {
                    Expr::Call(c) => &mut c.args,
                    Expr::MethodCall(c) => &mut c.args,
                    _ => unreachable!(),
                };
                for (i, argument) in args.iter_mut().enumerate() {
                    let modifiers: Vec<_> = patches
                        .iter()
                        .filter(|p| p.kind == Kind::ModifyArg && p.argument == Some(i))
                        .copied()
                        .collect();
                    *argument = value_chain(argument.clone(), &modifiers)?;
                }
                let wrappers: Vec<_> = patches
                    .iter()
                    .filter(|p| p.kind == Kind::WrapOperation)
                    .copied()
                    .collect();
                let bundles: Vec<_> = patches
                    .iter()
                    .filter(|p| p.kind == Kind::ModifyArgs)
                    .copied()
                    .collect();
                if !bundles.is_empty() {
                    new = bundle_arguments(&new, &bundles, &wrappers)?;
                } else if !wrappers.is_empty() {
                    new = wrap_operation(&new, &wrappers)?;
                }
                let after: Vec<_> = patches
                    .iter()
                    .filter(|p| p.kind == Kind::AfterCall)
                    .copied()
                    .collect();
                if !after.is_empty() {
                    new = after_chain(new, &after, &self.target_result, discarded)?;
                }
                for p in patches.iter().filter(|p| p.kind == Kind::AfterAssign) {
                    self.after_stack
                        .last_mut()
                        .ok_or_else(|| {
                            Diagnostic::new("E_PLAN_SITE", "AFTER_ASSIGN has no statement")
                        })?
                        .push(control_statement(p)?);
                }
                let before: Vec<_> = patches
                    .iter()
                    .filter(|p| p.kind == Kind::BeforeCall)
                    .map(|p| control_statement(p))
                    .collect::<Result<_>>()?;
                if !before.is_empty() {
                    new = if discarded {
                        syn::parse2(quote!({#(#before)* #new;}))
                    } else {
                        syn::parse2(quote!({#(#before)* #new}))
                    }
                    .map_err(syntax)?;
                }
                Ok::<_, Diagnostic>(new)
            })();
            match result {
                Ok(new) => {
                    *expr = new;
                    self.matched += 1;
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(states) = self.states.get(&expression) {
            let result = (|| {
                let Expr::Struct(literal) = expr else {
                    return Err(Diagnostic::new(
                        "E_PLAN_SITE",
                        "state constructor is not a literal",
                    ));
                };
                for (s, c) in states {
                    if c.fingerprint != expression_fingerprint {
                        return Err(Diagnostic::new(
                            "E_STALE_SOURCE",
                            "state literal fingerprint mismatch",
                        ));
                    }
                    // Struct update moves the existing state, preserving its lifecycle.
                    if literal.rest.is_none() {
                        let name: syn::Ident = syn::parse_str(&s.name).map_err(syntax)?;
                        let init = s.initializer()?;
                        literal.fields.push(syn::parse_quote!(#name: #init()));
                    }
                }
                Ok(())
            })();
            match result {
                Ok(()) => self.matched += 1,
                Err(e) => self.error = Some(e),
            }
        }
        self.modify_values(expr, expression, &expression_fingerprint);
    }
}
impl Rewrite<'_> {
    fn modify_values(&mut self, expr: &mut Expr, expression: usize, fingerprint: &str) {
        let variable = self.variables.get(&expression).cloned().unwrap_or_default();
        let modifiers = self
            .expressions
            .get(&expression)
            .cloned()
            .unwrap_or_default();
        if variable.is_empty() && modifiers.is_empty() {
            return;
        }
        if variable
            .iter()
            .chain(&modifiers)
            .any(|p| p.site_fingerprint.as_deref() != Some(fingerprint))
        {
            self.error = Some(Diagnostic::new(
                "E_STALE_SOURCE",
                "value expression fingerprint mismatch",
            ));
            return;
        }
        let load: Vec<_> = variable
            .iter()
            .filter(|p| {
                p.variable.as_ref().unwrap().at == horizon_mixins_core::manifest::Access::Load
            })
            .copied()
            .collect();
        let store: Vec<_> = variable
            .iter()
            .filter(|p| {
                p.variable.as_ref().unwrap().at == horizon_mixins_core::manifest::Access::Store
            })
            .copied()
            .collect();
        let result = (|| {
            let value = value_chain(expr.clone(), &load)?;
            let value = value_chain(value, &modifiers)?;
            value_chain(value, &store)
        })();
        match result {
            Ok(value) => {
                *expr = value;
                self.matched +=
                    usize::from(!variable.is_empty()) + usize::from(!modifiers.is_empty());
            }
            Err(e) => self.error = Some(e),
        }
    }
}

struct ReturnRewrite<'a> {
    patches: &'a [&'a PlannedInjection],
    error: Option<Diagnostic>,
}
impl VisitMut for ReturnRewrite<'_> {
    fn visit_item_mut(&mut self, _: &mut Item) {} // Generated helpers have their own returns.
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        if matches!(expr, Expr::Closure(_)) {
            return;
        } // Generated FnOnce continuations.
        visit_mut::visit_expr_mut(self, expr);
        if let Expr::Return(r) = expr {
            let value = r
                .expr
                .as_deref()
                .cloned()
                .unwrap_or_else(|| syn::parse_quote!(()));
            match value_chain(value, self.patches) {
                Ok(new) => r.expr = Some(Box::new(new)),
                Err(e) => self.error = Some(e),
            }
        }
    }
}
fn locate<'a>(items: &'a mut [Item], path: &[usize]) -> Result<(&'a Signature, &'a mut Block)> {
    let Some((first, rest)) = path.split_first() else {
        return Err(Diagnostic::new("E_PLAN_SITE", "empty AST path"));
    };
    let item = items
        .get_mut(*first)
        .ok_or_else(|| Diagnostic::new("E_PLAN_SITE", "invalid AST item index"))?;
    match item {
        Item::Fn(f) if rest.is_empty() => Ok((&f.sig, &mut f.block)),
        Item::Mod(m) => {
            if let Some((_, items)) = &mut m.content {
                locate(items, rest)
            } else {
                Err(Diagnostic::new("E_PLAN_SITE", "expected inline module"))
            }
        }
        Item::Impl(i) if rest.len() == 1 => match i.items.get_mut(rest[0]) {
            Some(syn::ImplItem::Fn(f)) => Ok((&f.sig, &mut f.block)),
            _ => Err(Diagnostic::new("E_PLAN_SITE", "expected impl method")),
        },
        _ => Err(Diagnostic::new(
            "E_PLAN_SITE",
            "AST path does not address a supported function",
        )),
    }
}

fn parent_items<'a>(items: &'a mut Vec<Item>, path: &[usize]) -> Result<&'a mut Vec<Item>> {
    let Some((first, rest)) = path.split_first() else {
        return Ok(items);
    };
    let Some(Item::Mod(module)) = items.get_mut(*first) else {
        return Err(Diagnostic::new("E_PLAN_SITE", "bridge module disappeared"));
    };
    let Some((_, children)) = &mut module.content else {
        return Err(Diagnostic::new(
            "E_PLAN_SITE",
            "bridge parent must be inline",
        ));
    };
    parent_items(children, rest)
}

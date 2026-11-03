use super::*;
fn fixture() -> Fixture {
    let f = Fixture::new();
    fs::remove_dir_all(&f.mods).unwrap();
    fs::create_dir_all(&f.mods).unwrap();
    fs::write(f.root.join("driver/src/lib.rs"), "").unwrap();
    f.append("engine/src/lib.rs", "\npub mod boundary;\n");
    fs::write(f.root.join("engine/src/boundary.rs"), r#"
use std::cell::RefCell;
use std::rc::Rc;
pub struct Host<'a> { pub log: &'a RefCell<Vec<&'static str>> }
pub struct Probe(pub Rc<RefCell<usize>>);
impl Drop for Probe { fn drop(&mut self) { *self.0.borrow_mut() += 1; } }
impl<'a> Host<'a> {
    pub fn run(&mut self, fail: bool, _probe: Probe) -> Result<i32, &'static str> {
        let work = || if fail { Err("failure") } else { Ok(3) };
        let value = work()?;
        self.log.borrow_mut().push("body");
        Ok(value)
    }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn preserved_closure_errors_and_drop() {
        for fail in [false,true] {
            let log = RefCell::new(Vec::new()); let dropped = Rc::new(RefCell::new(0));
            let result = Host { log: &log }.run(fail, Probe(dropped.clone()));
            assert_eq!(result, if fail {Err("failure")} else {Ok(3)});
            assert_eq!(*dropped.borrow(),1);
            assert_eq!(*log.borrow(), if fail {vec!["alpha","beta"]} else {vec!["alpha","beta","body"]});
        }
    }
}
"#).unwrap();
    for (id, priority) in [("alpha", 100), ("beta", 50)] {
        fs::create_dir(f.mods.join(id)).unwrap();
        fs::write(f.mods.join(id).join("head.rs"),format!("fn head(host: &mut Host<'_>) {{ host.log.borrow_mut().push(\"{id}\"); support::touch(); }}")).unwrap();
        fs::write(
            f.mods.join(id).join("horizon-mixin.toml"),
            format!(
                r#"
[mod]
id="{id}"
version="1"
priority={priority}
[[injection]]
id="head"
kind="HEAD"
target="fixture_game::boundary::Host<'_>::run"
helper="head.rs"
bindings=[{{name="host",ty="&mut Host<'_>",value="&mut *self"}}]
"#
            ),
        )
        .unwrap();
    }
    fs::write(f.mods.join("alpha/runtime.rs"), "pub fn touch() {}\n").unwrap();
    f.append(
        "mods/alpha/horizon-mixin.toml",
        r#"
[[module]]
id="runtime"
file="engine/src/boundary.rs"
name="support"
source="runtime.rs"
"#,
    );
    f
}
#[test]
fn recursion_limit_preserves_head_hooks_and_support_modules() {
    let f = fixture();
    for path in ["engine/src/lib.rs", "engine/src/boundary.rs"] {
        let file = f.root.join(path);
        let original = fs::read_to_string(&file).unwrap();
        fs::write(file, format!("#![recursion_limit = \"256\"]\n{original}")).unwrap();
    }
    let originals = source::files(&f.root).unwrap();
    let plan = build_plan(&f.root, &f.mods).unwrap();
    assert_eq!(plan.injections.len(), 2);
    assert_eq!(plan.modules.len(), 1);
    let report = f.run("check");
    assert!(
        report["cargo_stdout"]
            .as_str()
            .unwrap()
            .contains("preserved_closure_errors_and_drop ... ok")
    );
    for path in ["engine/src/lib.rs", "engine/src/boundary.rs"] {
        let generated = fs::read_to_string(f.out.join(path)).unwrap();
        assert!(generated.starts_with("#![recursion_limit = \"256\"]"));
    }
    assert_eq!(source::files(&f.root).unwrap(), originals);
    assert_eq!(f.run("check")["stage_changed"], false);
}

#[test]
fn unknown_file_attributes_remain_unsupported() {
    for path in ["engine/src/lib.rs", "engine/src/boundary.rs"] {
        let f = fixture();
        let file = f.root.join(path);
        let original = fs::read_to_string(&file).unwrap();
        fs::write(file, format!("#![not_a_builtin_attribute]\n{original}")).unwrap();
        assert_eq!(
            build_plan(&f.root, &f.mods).unwrap_err().code,
            "E_UNSUPPORTED_MACRO"
        );
    }
}

#[test]
fn head_preserves_lifetime_methods_closures_question_mark_and_drop() {
    let f = fixture();
    let original = source::files(&f.root).unwrap();
    let p = build_plan(&f.root, &f.mods).unwrap();
    assert_eq!(p.modules.len(), 1);
    let report = f.run("check");
    assert!(
        report["cargo_stdout"]
            .as_str()
            .unwrap()
            .contains("preserved_closure_errors_and_drop ... ok")
    );
    let snapshot = repo().join("crates/horizon-mixins-cli/tests/snapshots/boundary.rs");
    let generated = fs::read_to_string(f.out.join("engine/src/boundary.rs")).unwrap();
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        fs::write(&snapshot, &generated).unwrap();
    }
    assert_eq!(fs::read_to_string(snapshot).unwrap(), generated);
    let mut portable = p.clone();
    portable.rustc = "<pinned toolchain>".into();
    let planned = serde_json::to_string_pretty(&portable).unwrap() + "\n";
    let path = repo().join("crates/horizon-mixins-cli/tests/snapshots/boundary-plan.json");
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        fs::write(&path, &planned).unwrap();
    }
    assert_eq!(fs::read_to_string(path).unwrap(), planned);
    let mut stale = p.clone();
    stale.modules[0].fingerprint = "invalid".into();
    assert_eq!(
        horizon_mixins_transform::transform(&f.root, &stale)
            .unwrap_err()
            .code,
        "E_STALE_SOURCE"
    );
    let serialized = serde_json::to_vec(&p).unwrap();
    fs::rename(f.mods.join("alpha"), f.mods.join("zzz")).unwrap();
    assert_eq!(
        serialized,
        serde_json::to_vec(&build_plan(&f.root, &f.mods).unwrap()).unwrap()
    );
    let staged = source::files(&f.out).unwrap();
    let times = mtimes(&f.out);
    assert_eq!(f.run("check")["stage_changed"], false);
    assert_eq!(staged, source::files(&f.out).unwrap());
    assert_eq!(times, mtimes(&f.out));
    assert_eq!(original, source::files(&f.root).unwrap());
}
#[test]
fn support_module_explicit_crate_visibility_matches_the_default() {
    let f = fixture();
    let default = build_plan(&f.root, &f.mods).unwrap();
    f.append(
        "mods/alpha/horizon-mixin.toml",
        "visibility = \"pub(crate)\"\n",
    );
    let explicit = build_plan(&f.root, &f.mods).unwrap();
    assert_eq!(
        default.modules[0].visibility,
        horizon_mixins_core::manifest::ModuleVisibility::Crate
    );
    assert_eq!(
        serde_json::to_value(&default.modules).unwrap(),
        serde_json::to_value(&explicit.modules).unwrap()
    );
    assert_eq!(
        horizon_mixins_transform::transform(&f.root, &default).unwrap(),
        horizon_mixins_transform::transform(&f.root, &explicit).unwrap()
    );
    let roundtrip: horizon_mixins_core::Plan =
        serde_json::from_value(serde_json::to_value(&default).unwrap()).unwrap();
    assert_eq!(
        roundtrip.modules[0].visibility,
        default.modules[0].visibility
    );
    f.run("check");
}
#[test]
fn public_support_module_is_callable_from_a_hook_in_a_dependent_crate() {
    let f = Fixture::new();
    fs::remove_dir_all(&f.mods).unwrap();
    fs::create_dir_all(f.mods.join("alpha")).unwrap();
    fs::write(
        f.root.join("driver/src/lib.rs"),
        r#"
pub fn run(value: &mut i32) -> i32 { *value }
#[cfg(test)] mod tests {
    #[test] fn cross_crate_hook_calls_support_module() {
        assert_eq!(super::run(&mut 40), 42);
    }
}
"#,
    )
    .unwrap();
    fs::write(
        f.mods.join("alpha/horizon-mixin.toml"),
        r#"
[mod]
id="alpha"
version="1"
[[injection]]
id="head"
kind="HEAD"
target="fixture_driver::run"
helper="head.rs"
bindings=[{name="value",ty="&mut i32",value="&mut *value"}]
[[module]]
id="runtime"
file="engine/src/lib.rs"
name="support"
source="runtime.rs"
"#,
    )
    .unwrap();
    fs::write(
        f.mods.join("alpha/head.rs"),
        "fn head(value: &mut i32) { fixture_game::support::touch(value); }",
    )
    .unwrap();
    fs::write(
        f.mods.join("alpha/runtime.rs"),
        "pub fn touch(value: &mut i32) { *value += 2; }",
    )
    .unwrap();
    let original = source::files(&f.root).unwrap();
    let private = f.command("check").output().unwrap();
    assert!(!private.status.success());
    let error: serde_json::Value = serde_json::from_slice(&private.stderr).unwrap();
    assert_eq!(error["code"], "E_BUILD");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("module `support` is private")
    );

    f.append("mods/alpha/horizon-mixin.toml", "visibility = \"pub\"\n");
    let plan = build_plan(&f.root, &f.mods).unwrap();
    let serialized = serde_json::to_value(&plan).unwrap();
    assert_eq!(serialized["modules"][0]["visibility"], "pub");
    let roundtrip: horizon_mixins_core::Plan = serde_json::from_value(serialized).unwrap();
    assert_eq!(
        horizon_mixins_transform::transform(&f.root, &plan).unwrap(),
        horizon_mixins_transform::transform(&f.root, &roundtrip).unwrap()
    );
    let report = f.run("check");
    assert_eq!(report["stage_changed"], true);
    assert!(
        report["cargo_stdout"]
            .as_str()
            .unwrap()
            .contains("cross_crate_hook_calls_support_module ... ok")
    );
    let staged = source::files(&f.out).unwrap();
    let times = mtimes(&f.out);
    assert_eq!(f.run("check")["stage_changed"], false);
    assert_eq!(source::files(&f.out).unwrap(), staged);
    assert_eq!(mtimes(&f.out), times);
    assert_eq!(source::files(&f.root).unwrap(), original);
}
#[test]
fn support_module_visibility_rejects_unknown_values() {
    for visibility in [
        "public",
        "private",
        "pub(super)",
        "pub(in crate::runtime)",
        "",
    ] {
        let f = fixture();
        f.append(
            "mods/alpha/horizon-mixin.toml",
            &format!("visibility = {visibility:?}\n"),
        );
        let error = build_plan(&f.root, &f.mods).unwrap_err();
        assert_eq!(error.code, "E_MANIFEST");
        assert!(error.message.contains("pub(crate)"));
        let f = fixture();
        let mut plan = serde_json::to_value(build_plan(&f.root, &f.mods).unwrap()).unwrap();
        plan["modules"][0]["visibility"] = visibility.into();
        assert!(serde_json::from_value::<horizon_mixins_core::Plan>(plan).is_err());
    }
}
#[test]
fn cancellable_head_skips_original_lifetime_body_and_drops_inputs_once() {
    let f = fixture();
    fs::write(f.mods.join("alpha/head.rs"),"fn head(host:&mut Host<'_>)->core::ops::ControlFlow<Result<i32,&'static str>> {host.log.borrow_mut().push(\"alpha\");core::ops::ControlFlow::Break(Ok(9))}").unwrap();
    f.replace(
        "mods/alpha/horizon-mixin.toml",
        "kind=\"HEAD\"",
        "kind=\"HEAD\"\ncancellable=true",
    );
    f.replace(
        "engine/src/boundary.rs",
        "if fail {Err(\"failure\")} else {Ok(3)}",
        "Ok(9)",
    );
    f.replace(
        "engine/src/boundary.rs",
        "if fail {vec![\"alpha\",\"beta\"]} else {vec![\"alpha\",\"beta\",\"body\"]}",
        "vec![\"alpha\"]",
    );
    f.run("check");
}
#[test]
fn support_modules_fail_closed_for_conflicts_external_sources_and_bad_paths() {
    for (source, code) in [
        ("mod outside;", "E_MODULE"),
        ("include!(\"secret.rs\");", "E_MODULE"),
        ("#![allow(unused)]\nfn x(){}", "E_MODULE"),
    ] {
        let f = fixture();
        fs::write(f.mods.join("alpha/runtime.rs"), source).unwrap();
        assert_eq!(build_plan(&f.root, &f.mods).unwrap_err().code, code);
    }
    for declaration in [
        "use std::fmt as support;",
        "use std::fmt::*;",
        "#[path=\"boundary.rs\"] pub mod reused;",
    ] {
        let f = fixture();
        if declaration.contains("reused") {
            f.append("engine/src/lib.rs", declaration);
        } else {
            f.append("engine/src/boundary.rs", declaration);
        }
        let code = build_plan(&f.root, &f.mods).unwrap_err().code;
        assert!(code == "E_MODULE_CONFLICT" || code == "E_TARGET_AMBIGUOUS");
    }
    let f = fixture();
    f.append("engine/src/boundary.rs", "mod support {}\n");
    assert_eq!(
        build_plan(&f.root, &f.mods).unwrap_err().code,
        "E_MODULE_CONFLICT"
    );
    let f = fixture();
    f.replace(
        "mods/alpha/horizon-mixin.toml",
        "engine/src/boundary.rs",
        "../escape.rs",
    );
    assert_eq!(
        build_plan(&f.root, &f.mods).unwrap_err().code,
        "E_INVALID_PATH"
    );
    let f = fixture();
    f.replace(
        "mods/alpha/horizon-mixin.toml",
        "id=\"runtime\"",
        "id=\"head\"",
    );
    assert_eq!(
        build_plan(&f.root, &f.mods).unwrap_err().code,
        "E_DUPLICATE_ID"
    );
}
#[test]
fn lifetime_identity_is_stable_but_type_generics_and_body_rewrites_remain_rejected() {
    let f = fixture();
    let p = serde_json::to_value(build_plan(&f.root, &f.mods).unwrap()).unwrap();
    f.replace(
        "engine/src/boundary.rs",
        "impl<'a> Host<'a>",
        "impl<'renamed> Host<'renamed>",
    );
    let next = build_plan(&f.root, &f.mods).unwrap();
    assert_eq!(p["injections"][0]["target"], next.injections[0].target);
    let f = fixture();
    f.replace("engine/src/boundary.rs", "pub fn run(", "pub fn run<T>(");
    assert_eq!(
        build_plan(&f.root, &f.mods).unwrap_err().code,
        "E_UNSUPPORTED_SIGNATURE"
    );
    let f = fixture();
    f.append(
        "mods/alpha/horizon-mixin.toml",
        r#"
[[injection]]
id="return"
kind="RETURN"
target="fixture_game::boundary::Host<'_>::run"
helper="return.rs"
bindings=[{name="value",ty="Result<i32,&'static str>",value="$value"}]
"#,
    );
    fs::write(
        f.mods.join("alpha/return.rs"),
        "fn ret(value:Result<i32,&'static str>)->Result<i32,&'static str>{value}",
    )
    .unwrap();
    assert_eq!(
        build_plan(&f.root, &f.mods).unwrap_err().code,
        "E_UNSUPPORTED_CONTROL_FLOW"
    );
}

fn macro_fixture() -> Fixture {
    let f = fixture();
    f.replace(
        "engine/src/boundary.rs",
        "impl<'a> Host<'a> {",
        r#"
macro_rules! stop { () => { return Err("failure") }; }
macro_rules! finish { ($value:expr) => { $value }; }
struct Guard<'a>(&'a RefCell<Vec<&'static str>>);
impl Drop for Guard<'_> {
    fn drop(&mut self) { self.0.borrow_mut().push("local_drop"); }
}
impl<'a> Host<'a> {"#,
    );
    f.replace(
        "engine/src/boundary.rs",
        "let work = || if fail { Err(\"failure\") } else { Ok(3) };\n        let value = work()?;",
        r#"let _guard = Guard(self.log);
        assert!(std::matches!(fail, false | true));
        let evaluations = std::cell::Cell::new(0);
        let values = vec![
            { evaluations.set(evaluations.get() + 1); 2 },
            { evaluations.set(evaluations.get() + 1); 3 },
        ];
        let repeated = std::vec![{ evaluations.set(evaluations.get() + 1); values[0] }; 9];
        let text = format!("{}", { evaluations.set(evaluations.get() + 1); repeated[0] + values[1] });
        assert_eq!(evaluations.get(), 4);
        assert_eq!(repeated.len(), 9);
        assert!(matches!(text.as_str(), "5"));
        if matches!(fail, true) { stop!(); }
        let value = 3;"#,
    );
    f.replace("engine/src/boundary.rs", "Ok(value)", "finish!(Ok(value))");
    f.replace(
        "engine/src/boundary.rs",
        "if fail {vec![\"alpha\",\"beta\"]} else {vec![\"alpha\",\"beta\",\"body\"]}",
        "if fail {vec![\"alpha\",\"beta\",\"local_drop\"]} else {vec![\"alpha\",\"beta\",\"body\",\"local_drop\"]}",
    );
    f
}

#[test]
fn head_preserves_body_macros_early_returns_and_drop() {
    let f = macro_fixture();
    let original = source::files(&f.root).unwrap();
    let plan = build_plan(&f.root, &f.mods).unwrap();
    let generated = horizon_mixins_transform::transform(&f.root, &plan).unwrap();
    // Removing only the two HEAD calls must reproduce the original body,
    // including every opaque macro token.
    let file = "engine/src/boundary.rs";
    fn run_body(text: &[u8]) -> syn::Block {
        let tree = syn::parse_file(std::str::from_utf8(text).unwrap()).unwrap();
        tree.items
            .into_iter()
            .find_map(|item| {
                let syn::Item::Impl(item) = item else {
                    return None;
                };
                item.items.into_iter().find_map(|item| match item {
                    syn::ImplItem::Fn(f) if f.sig.ident == "run" => Some(f.block),
                    _ => None,
                })
            })
            .unwrap()
    }
    let mut body = run_body(&generated[file]);
    body.stmts.drain(..2);
    assert_eq!(
        horizon_mixins_core::index::tokens(&body),
        horizon_mixins_core::index::tokens(&run_body(&original[file])),
    );
    let report = f.run("check");
    assert!(
        report["cargo_stdout"]
            .as_str()
            .unwrap()
            .contains("preserved_closure_errors_and_drop ... ok")
    );
    let times = mtimes(&f.out);
    assert_eq!(f.run("check")["stage_changed"], false);
    assert_eq!(times, mtimes(&f.out));
    assert_eq!(original, source::files(&f.root).unwrap());
}

#[test]
fn cancellable_head_preserves_macros_on_continue_and_skips_them_on_break() {
    for cancel in [false, true] {
        let f = macro_fixture();
        f.replace(
            "mods/alpha/horizon-mixin.toml",
            "kind=\"HEAD\"",
            "kind=\"HEAD\"\ncancellable=true",
        );
        let flow = if cancel {
            "core::ops::ControlFlow::Break(Ok(9))"
        } else {
            "core::ops::ControlFlow::Continue(())"
        };
        fs::write(
            f.mods.join("alpha/head.rs"),
            format!("fn head(host: &mut Host<'_>) -> core::ops::ControlFlow<Result<i32, &'static str>> {{ host.log.borrow_mut().push(\"alpha\"); {flow} }}"),
        ).unwrap();
        if cancel {
            // If any original macro executes, this deliberately fails. The
            // skipped local Guard must not be constructed or dropped either.
            f.replace("engine/src/boundary.rs", "false | true", "false if false");
            f.replace(
                "engine/src/boundary.rs",
                "if fail {Err(\"failure\")} else {Ok(3)}",
                "Ok(9)",
            );
            f.replace(
                "engine/src/boundary.rs",
                "if fail {vec![\"alpha\",\"beta\",\"local_drop\"]} else {vec![\"alpha\",\"beta\",\"body\",\"local_drop\"]}",
                "vec![\"alpha\"]",
            );
        }
        let original = source::files(&f.root).unwrap();
        f.run("check");
        assert_eq!(f.run("check")["stage_changed"], false);
        assert_eq!(original, source::files(&f.root).unwrap());
    }
}

#[test]
fn head_macro_exception_keeps_cfg_attributes_and_other_macro_contexts_restricted() {
    for (from, to, code) in [
        (
            "pub fn run(",
            "#[instrument]\npub fn run(",
            "E_UNSUPPORTED_MACRO",
        ),
        (
            "impl<'a> Host<'a>",
            "#[instrument]\nimpl<'a> Host<'a>",
            "E_UNSUPPORTED_MACRO",
        ),
        (
            "let value = work()?;",
            "#[cfg(any())] assert!(true); let value = work()?;",
            "E_UNSUPPORTED_CFG",
        ),
        (
            "let value = work()?;",
            "#[cfg_attr(any(), allow(unused))] assert!(true); let value = work()?;",
            "E_UNSUPPORTED_CFG",
        ),
        (
            "let value = work()?;",
            "#[instrument] assert!(true); let value = work()?;",
            "E_UNSUPPORTED_MACRO",
        ),
        (
            "let value = work()?;",
            "let value = #[instrument] matches!(fail, true);",
            "E_UNSUPPORTED_MACRO",
        ),
        (
            "let value = work()?;",
            "let value = #[cfg(any())] matches!(fail, true);",
            "E_UNSUPPORTED_CFG",
        ),
        (
            "let value = work()?;",
            "let pattern!() = work()?;",
            "E_UNSUPPORTED_MACRO",
        ),
        (
            "fail: bool",
            "fail: parameter_type!()",
            "E_UNSUPPORTED_MACRO",
        ),
        (
            "let value = work()?;",
            "let value: body_type!() = work()?;",
            "E_UNSUPPORTED_MACRO",
        ),
        (
            "let value = work()?;",
            "macro_rules! local { () => { 3 }; } let value = work()?;",
            "E_UNSUPPORTED_MACRO",
        ),
    ] {
        for cancellable in [false, true] {
            let f = fixture();
            f.replace("engine/src/boundary.rs", from, to);
            if cancellable {
                f.replace(
                    "mods/alpha/horizon-mixin.toml",
                    "kind=\"HEAD\"",
                    "kind=\"HEAD\"\ncancellable=true",
                );
            }
            let error = build_plan(&f.root, &f.mods)
                .err()
                .unwrap_or_else(|| panic!("expected rejection of {to}"));
            assert_eq!(error.code, code, "{to}");
        }
    }
}

#[test]
fn head_with_opaque_macros_cannot_compose_with_return_or_interior_hooks() {
    for declaration in [
        "kind=\"RETURN\"",
        "kind=\"BEFORE_CALL\"\nselector=\"work\"\nrequire=1\nordinal=0",
    ] {
        let f = fixture();
        // Keep the body otherwise inside the stricter subset, so the macro
        // itself is what prevents body analysis, regardless of mod order.
        f.replace(
            "engine/src/boundary.rs",
            "let work = || if fail { Err(\"failure\") } else { Ok(3) };\n        let value = work()?;",
            "assert!(true); let value = 3;",
        );
        f.append(
            "mods/beta/horizon-mixin.toml",
            &format!(
                r#"
[[injection]]
id="body"
{declaration}
target="fixture_game::boundary::Host<'_>::run"
helper="head.rs"
bindings=[{{name="host",ty="&mut Host<'_>",value="&mut *self"}}]
"#
            ),
        );
        assert_eq!(
            build_plan(&f.root, &f.mods).unwrap_err().code,
            "E_UNSUPPORTED_MACRO"
        );
    }
}

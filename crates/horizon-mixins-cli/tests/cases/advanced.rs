use super::*;
use horizon_mixins_core::manifest::Kind;
use serde_json::json;
fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("original");
    let mods = temp.path().join("mods");
    copy(&repo().join("fixtures/advanced"), &root);
    copy(&repo().join("fixtures/advanced-mods"), &mods);
    Fixture {
        out: temp.path().join("stage"),
        _temp: temp,
        root,
        mods,
    }
}
fn edit(
    f: &Fixture,
    bundle: &str,
    id: &str,
    change: impl FnOnce(&mut toml::map::Map<String, toml::Value>),
) {
    let path = f.mods.join(bundle).join("horizon-mixin.toml");
    let mut manifest: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let injection = manifest["injection"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|i| i["id"].as_str() == Some(id))
        .unwrap();
    change(injection.as_table_mut().unwrap());
    fs::write(path, toml::to_string(&manifest).unwrap()).unwrap();
}
fn field(f: &Fixture, bundle: &str, id: &str, name: &str, value: toml::Value) {
    edit(f, bundle, id, |i| {
        i.insert(name.into(), value);
    });
}
fn failure(f: &Fixture, code: &str) {
    let before = source::files(&f.root).unwrap();
    let error = build_plan(&f.root, &f.mods).unwrap_err();
    assert_eq!(error.code, code, "{error:?}");
    assert!(error.file.is_some(), "{error:?}");
    assert_eq!(source::files(&f.root).unwrap(), before);
    assert!(!f.out.exists());
}
#[test]
fn all_features_compile_compose_and_preserve_source_and_mtimes() {
    let f = fixture();
    let original = source::files(&f.root).unwrap();
    let times = mtimes(&f.root);
    let report = f.run("check");
    assert!(
        report["cargo_stdout"]
            .as_str()
            .unwrap()
            .contains("8 passed")
    );
    let first = source::files(&f.out).unwrap();
    let stage_times = mtimes(&f.out);
    assert_eq!(f.run("check")["stage_changed"], false);
    assert_eq!(source::files(&f.out).unwrap(), first);
    assert_eq!(mtimes(&f.out), stage_times);
    assert_eq!(source::files(&f.root).unwrap(), original);
    assert_eq!(mtimes(&f.root), times);
}
#[test]
fn advanced_source_and_plan_snapshots_ignore_discovery_order() {
    let f = fixture();
    let plan = build_plan(&f.root, &f.mods).unwrap();
    assert_eq!(plan.schema, 7);
    fs::rename(f.mods.join("alpha"), f.mods.join("z-alpha")).unwrap();
    fs::rename(f.mods.join("beta"), f.mods.join("a-beta")).unwrap();
    assert_eq!(
        serde_json::to_vec(&plan).unwrap(),
        serde_json::to_vec(&build_plan(&f.root, &f.mods).unwrap()).unwrap()
    );
    let output = horizon_mixins_transform::transform(&f.root, &plan).unwrap();
    let mut summary = serde_json::to_value(&plan).unwrap();
    summary.as_object_mut().unwrap().remove("inputs");
    summary.as_object_mut().unwrap().remove("rustc");
    let snapshots = repo().join("crates/horizon-mixins-cli/tests/snapshots");
    for (name, bytes) in [
        ("advanced.rs", output["game/src/lib.rs"].clone()),
        (
            "advanced-plan.json",
            (serde_json::to_string_pretty(&summary).unwrap() + "\n").into_bytes(),
        ),
    ] {
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            fs::write(snapshots.join(name), &bytes).unwrap();
        }
        assert_eq!(bytes, fs::read(snapshots.join(name)).unwrap(), "{name}");
    }
}
#[test]
fn new_manifest_fields_and_value_contracts_fail_closed() {
    let cases = [
        (
            "cancel",
            "cancellable",
            toml::Value::Boolean(false),
            "E_INVALID_HELPER",
        ),
        (
            "return",
            "cancellable",
            toml::Value::Boolean(true),
            "E_MANIFEST",
        ),
        (
            "return",
            "selector",
            toml::Value::String("anything".into()),
            "E_SELECTOR",
        ),
        (
            "argument-number",
            "argument",
            toml::Value::Integer(99),
            "E_ARGUMENT",
        ),
        (
            "argument-number",
            "selector",
            toml::Value::String("missing".into()),
            "E_MATCH_COUNT",
        ),
        (
            "constant",
            "selector",
            toml::Value::String("3i32".into()),
            "E_MATCH_COUNT",
        ),
        (
            "negative",
            "selector",
            toml::Value::String("2".into()),
            "E_MATCH_COUNT",
        ),
        (
            "constant",
            "selector",
            toml::Value::String("SOME_CONST".into()),
            "E_SELECTOR",
        ),
        ("constant", "require", toml::Value::Integer(0), "E_SELECTOR"),
        ("constant", "ordinal", toml::Value::Integer(9), "E_ORDINAL"),
        (
            "array",
            "selector",
            toml::Value::String("2".into()),
            "E_UNSUPPORTED_SITE",
        ),
        (
            "wrap-owned",
            "require",
            toml::Value::Integer(2),
            "E_MATCH_COUNT",
        ),
        (
            "wrap-owned",
            "ordinal",
            toml::Value::Integer(1),
            "E_ORDINAL",
        ),
    ];
    for (id, name, value, code) in cases {
        let f = fixture();
        field(&f, "alpha", id, name, value);
        failure(&f, code);
    }
    let f = fixture();
    edit(&f, "alpha", "argument-number", |i| {
        i.remove("argument");
    });
    failure(&f, "E_ARGUMENT");
    let f = fixture();
    edit(&f, "alpha", "owned-return", |i| {
        i["bindings"].as_array_mut().unwrap()[0]["value"] = "input".into();
    });
    failure(&f, "E_INVALID_BINDING");
    let f = fixture();
    edit(&f, "alpha", "exit-return", |i| {
        i["bindings"].as_array_mut().unwrap()[0]["ty"] = "i64".into();
    });
    failure(&f, "E_INVALID_BINDING");
    for (path, from, to, code) in [
        (
            "mods/alpha/cancel.rs",
            "ControlFlow<i32>",
            "ControlFlow<i32, bool>",
            "E_INVALID_HELPER",
        ),
        (
            "mods/alpha/exit-return.rs",
            "-> i32",
            "-> i64",
            "E_INVALID_HELPER",
        ),
        (
            "mods/alpha/argument-owned.rs",
            "-> String",
            "-> i32",
            "E_INVALID_HELPER",
        ),
        (
            "game/src/lib.rs",
            "pub fn constants() -> i32",
            "pub async fn constants() -> i32",
            "E_UNSUPPORTED_SIGNATURE",
        ),
        (
            "game/src/lib.rs",
            "pub fn constants() -> i32 { 3",
            "pub fn constants() -> i32 { let _c = || 0; 3",
            "E_UNSUPPORTED_CONTROL_FLOW",
        ),
    ] {
        let f = fixture();
        f.replace(path, from, to);
        failure(&f, code);
    }
}
#[test]
fn wrapper_signatures_receiver_borrows_and_continuations_are_validated() {
    let f = fixture();
    edit(&f, "alpha", "wrap-owned", |i| {
        i["bindings"].as_array_mut().unwrap().remove(0);
    });
    failure(&f, "E_INVALID_BINDING");
    let f = fixture();
    edit(&f, "alpha", "wrap-owned", |i| {
        i["bindings"].as_array_mut().unwrap()[0]["value"] = "$arg1".into();
    });
    failure(&f, "E_INVALID_BINDING");
    let f = fixture();
    edit(&f, "alpha", "wrap-owned", |i| {
        i["bindings"].as_array_mut().unwrap()[2]["ty"] =
            "impl FnMut(String, Trace) -> String".into();
    });
    f.replace("mods/alpha/wrap-owned.rs", "FnOnce", "FnMut");
    failure(&f, "E_INVALID_BINDING");
    let f = fixture();
    f.replace(
        "mods/alpha/wrap-owned.rs",
        ") -> String {",
        ") -> &'static str {",
    );
    failure(&f, "E_OPERATION_SIGNATURE");
    let f = fixture();
    f.replace("game/src/lib.rs", "original_owned(make_owned(evaluations, &trace), trace)", "let original_owned = original_owned;\n    original_owned(make_owned(evaluations, &trace), trace)");
    failure(&f, "E_UNSUPPORTED_SITE");
    let f = fixture();
    f.replace(
        "game/src/lib.rs",
        "counter.apply_traced(value, trace)",
        "counter.apply_traced(counter.total, trace)",
    );
    failure(&f, "E_UNSUPPORTED_SITE");
    let f = fixture();
    f.replace(
        "game/src/lib.rs",
        "wrapping_method(counter: &mut Counter",
        "wrapping_method(counter: Counter",
    );
    failure(&f, "E_UNSUPPORTED_SITE");
    let f = fixture();
    f.replace(
        "game/src/lib.rs",
        "counter.apply_traced(value, trace)",
        "let counter = counter;\n    counter.apply_traced(value, trace)",
    );
    failure(&f, "E_INVALID_BINDING");
    let f = fixture();
    f.replace("mods/alpha/wrap-method.rs", "value: i32", "value: i64");
    f.replace(
        "mods/alpha/wrap-method.rs",
        "FnOnce(&mut Counter, i32, Trace)",
        "FnOnce(&mut Counter, i64, Trace)",
    );
    edit(&f, "alpha", "wrap-method", |i| {
        i["bindings"].as_array_mut().unwrap()[1]["ty"] = "i64".into();
        i["bindings"].as_array_mut().unwrap()[3]["ty"] =
            "impl FnOnce(&mut Counter, i64, Trace) -> i32".into();
    });
    failure(&f, "E_OPERATION_SIGNATURE");
}
#[test]
fn competing_value_signatures_and_overwrites_fail_before_staging() {
    let f = fixture();
    fs::write(
        f.mods.join("beta/argument-owned.rs"),
        "fn modify(value: i32, log: &mut Vec<&'static str>) -> i32 { let _ = log; value + 1 }",
    )
    .unwrap();
    edit(&f, "beta", "argument-owned", |i| {
        i["bindings"].as_array_mut().unwrap()[0]["ty"] = "i32".into();
    });
    failure(&f, "E_VALUE_SIGNATURE");
    let error = build_plan(&f.root, &f.mods).unwrap_err();
    assert!(
        error.mod_id.is_some()
            && error.injection_id.is_some()
            && error.target.is_some()
            && error.selector.is_some()
    );
    assert!(error.message.contains("alpha/") && error.message.contains("beta/"));
    let f = fixture();
    f.replace("mods/alpha/combined-argument.rs", "i32", "i64");
    edit(&f, "alpha", "combined-argument", |i| {
        i["bindings"].as_array_mut().unwrap()[0]["ty"] = "i64".into();
    });
    failure(&f, "E_OPERATION_SIGNATURE");
    for (id, kind) in [
        ("cancel", "HEAD"),
        ("return", "RETURN"),
        ("argument-owned", "MODIFY_ARG"),
        ("constant", "MODIFY_CONSTANT"),
        ("wrap-owned", "WRAP_OPERATION"),
    ] {
        let f = fixture();
        let parsed: toml::Value =
            toml::from_str(&fs::read_to_string(f.mods.join("alpha/horizon-mixin.toml")).unwrap())
                .unwrap();
        let injection = parsed["injection"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"].as_str() == Some(id))
            .unwrap();
        let target = injection["target"].as_str().unwrap();
        f.append("mods/alpha/horizon-mixin.toml", &format!("\n[[injection]]\nid=\"exclusive\"\ntarget=\"{target}\"\nkind=\"OVERWRITE\"\nhelper=\"exclusive.rs\"\n"));
        fs::write(
            f.mods.join("alpha/exclusive.rs"),
            "fn replacement() -> i32 { 0 }",
        )
        .unwrap();
        assert_eq!(
            build_plan(&f.root, &f.mods).unwrap_err().code,
            "E_OVERWRITE_CONFLICT",
            "{kind}"
        );
        let output = f.command("apply").output().unwrap();
        assert!(!output.status.success());
        assert!(!f.out.exists());
    }
}
#[test]
fn compiler_enforces_fn_once_and_original_argument_types() {
    for (path, from, to, message) in [
        (
            "mods/alpha/wrap-owned.rs",
            "let output = original(value + \"a\", trace);",
            "let output = original(value, trace.clone()); original(String::new(), trace);",
            "moved value",
        ),
        (
            "mods/alpha/argument-owned.rs",
            "log.push(\"modify-alpha\"); value + \"a\"",
            "let moved = value; log.push(\"modify-alpha\"); value + &moved",
            "moved value",
        ),
    ] {
        let f = fixture();
        f.replace(path, from, to);
        let original = source::files(&f.root).unwrap();
        let output = f.command("check").output().unwrap();
        assert!(!output.status.success());
        let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["code"], "E_BUILD");
        assert!(
            error["message"].as_str().unwrap().contains(message),
            "{error}"
        );
        assert_eq!(source::files(&f.root).unwrap(), original);
    }
    let f = fixture();
    f.replace("mods/alpha/argument-number.rs", "i32", "f64");
    edit(&f, "alpha", "argument-number", |i| {
        i["bindings"].as_array_mut().unwrap()[0]["ty"] = "f64".into();
    });
    let output = f.command("check").output().unwrap();
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "E_BUILD");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("mismatched types")
    );
}
#[test]
fn malformed_new_plan_fields_return_diagnostics() {
    let f = fixture();
    let plan = build_plan(&f.root, &f.mods).unwrap();
    for field in ["constant_site", "argument"] {
        let mut json = serde_json::to_value(&plan).unwrap();
        let kind = if field == "constant_site" {
            "MODIFY_CONSTANT"
        } else {
            "MODIFY_ARG"
        };
        json["injections"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|p| p["kind"] == kind)
            .unwrap()[field] = json!(null);
        let bad = serde_json::from_value(json).unwrap();
        assert_eq!(
            horizon_mixins_transform::transform(&f.root, &bad)
                .unwrap_err()
                .code,
            "E_PLAN_SITE"
        );
    }
    assert!(
        plan.injections
            .iter()
            .any(|p| p.kind == Kind::WrapOperation)
    );
}

#[test]
fn modifiers_keep_temporaries_alive_and_returns_in_generated_helpers_untouched() {
    let f = fixture();
    f.append(
        "game/src/lib.rs",
        r#"
struct Temporary { trace: Trace }
impl Drop for Temporary { fn drop(&mut self) { self.trace.borrow_mut().push("temporary-drop"); } }
impl Temporary { fn value(&self) -> i32 { self.trace.borrow_mut().push("value"); 1 } }
fn last(trace: &Trace) -> i32 { trace.borrow_mut().push("last"); 2 }
fn add(a: i32, b: i32) -> i32 { a + b }
pub fn temporary_argument(trace: &Trace) -> i32 {
    add(Temporary { trace: trace.clone() }.value(), last(trace))
}
pub fn temporary_return(trace: &Trace) -> i32 {
    let _guard = Guard { trace: trace.clone() };
    return Temporary { trace: trace.clone() }.value();
}
pub fn nested_return(early: bool, trace: &Trace) -> i32 {
    base(if early { return 3; } else { 4 })
}
pub fn cancelled_unit(log: &mut Vec<&'static str>) { log.push("should-not-run"); }
"#,
    );
    f.append(
        "driver/src/lib.rs",
        r#"
#[test]
fn temporary_scopes_and_nested_returns() {
    let trace = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    assert_eq!(advanced_game::temporary_argument(&trace), 4);
    assert_eq!(*trace.borrow(), ["value", "modify", "last", "temporary-drop"]);
    trace.borrow_mut().clear();
    assert_eq!(advanced_game::temporary_return(&trace), 2);
    assert_eq!(*trace.borrow(), ["value", "modify", "temporary-drop", "local-drop"]);
    for (early, expected) in [(true, 4), (false, 13)] {
        trace.borrow_mut().clear();
        assert_eq!(advanced_game::nested_return(early, &trace), expected);
        assert_eq!(*trace.borrow(), ["modify"]);
    }
    let mut log = Vec::new(); advanced_game::cancelled_unit(&mut log);
    assert_eq!(log, ["cancel", "return"]);
}
"#,
    );
    fs::write(f.mods.join("alpha/temporary.rs"), "fn modify(value: i32, trace: &Trace) -> i32 { trace.borrow_mut().push(\"modify\"); value + 1 }").unwrap();
    for (id, target, kind, site) in [
        (
            "temporary-arg",
            "temporary_argument",
            "MODIFY_ARG",
            "selector=\"add\"\nrequire=1\nordinal=0\nargument=0\n",
        ),
        ("temporary-ret", "temporary_return", "RETURN", ""),
        ("nested-ret", "nested_return", "RETURN", ""),
    ] {
        f.append("mods/alpha/horizon-mixin.toml", &format!("\n[[injection]]\nid=\"{id}\"\ntarget=\"advanced_game::{target}\"\nkind=\"{kind}\"\n{site}helper=\"temporary.rs\"\nbindings=[{{name=\"value\",ty=\"i32\",value=\"$value\"}},{{name=\"trace\",ty=\"&Trace\",value=\"trace\"}}]\n"));
    }
    fs::write(f.mods.join("alpha/nested-wrap-extra.rs"), "fn around(value: i32, original: impl FnOnce(i32) -> i32) -> i32 { return original(value); }").unwrap();
    f.append("mods/alpha/horizon-mixin.toml", "\n[[injection]]\nid=\"nested-wrap-extra\"\ntarget=\"advanced_game::nested_return\"\nkind=\"WRAP_OPERATION\"\nselector=\"base\"\nrequire=1\nordinal=0\nhelper=\"nested-wrap-extra.rs\"\nbindings=[{name=\"value\",ty=\"i32\",value=\"$arg0\"},{name=\"original\",ty=\"impl FnOnce(i32) -> i32\",value=\"$operation\"}]\n");
    fs::write(f.mods.join("alpha/cancel-unit.rs"), "fn cancel(log: &mut Vec<&'static str>) -> core::ops::ControlFlow<()> { log.push(\"cancel\"); core::ops::ControlFlow::Break(()) }").unwrap();
    for (id, kind, file, extra, bindings) in [
        (
            "cancel-unit",
            "HEAD",
            "cancel-unit.rs",
            "cancellable=true\n",
            "[{name=\"log\",ty=\"&mut Vec<&'static str>\",value=\"&mut *log\"}]",
        ),
        (
            "return-unit",
            "RETURN",
            "unit-return.rs",
            "",
            "[{name=\"value\",ty=\"()\",value=\"$value\"},{name=\"log\",ty=\"&mut Vec<&'static str>\",value=\"&mut *log\"}]",
        ),
    ] {
        f.append("mods/alpha/horizon-mixin.toml", &format!("\n[[injection]]\nid=\"{id}\"\ntarget=\"advanced_game::cancelled_unit\"\nkind=\"{kind}\"\nhelper=\"{file}\"\n{extra}bindings={bindings}\n"));
    }
    assert!(
        f.run("check")["cargo_stdout"]
            .as_str()
            .unwrap()
            .contains("9 passed")
    );
}

#[test]
fn return_exit_proofs_handle_semicolons_and_reject_ambiguous_unit_aliases() {
    let f = fixture();
    f.replace(
        "game/src/lib.rs",
        "match input { 1 => 6, _ => { return 7; } }",
        "if input == 1 { return 6; } else { return 7; };",
    );
    assert!(
        f.run("check")["cargo_stdout"]
            .as_str()
            .unwrap()
            .contains("8 passed")
    );
    let f = fixture();
    f.append("game/src/lib.rs", "\ntype AliasUnit = ();\npub fn alias_unit(log: &mut Vec<&'static str>) -> AliasUnit { log.push(\"body\"); }\n");
    field(
        &f,
        "alpha",
        "unit-return",
        "target",
        "advanced_game::alias_unit".into(),
    );
    edit(&f, "alpha", "unit-return", |i| {
        i["bindings"].as_array_mut().unwrap()[0]["ty"] = "AliasUnit".into();
    });
    f.replace("mods/alpha/unit-return.rs", "value: ()", "value: AliasUnit");
    f.replace("mods/alpha/unit-return.rs", ") {", ") -> AliasUnit {");
    failure(&f, "E_UNSUPPORTED_SITE");
    f.replace(
        "game/src/lib.rs",
        "{ log.push(\"body\"); }",
        "{ log.push(\"body\"); () }",
    );
    build_plan(&f.root, &f.mods).unwrap();
    success(
        f.command("check")
            .args(["--package", "advanced-game"])
            .output()
            .unwrap(),
    );
}

#[test]
fn call_selectors_preserve_spaces_inside_receiver_literals() {
    for (selected, expected) in [
        ("two words", ["before", "two words", "twowords"]),
        ("twowords", ["two words", "before", "twowords"]),
    ] {
        let f = fixture();
        f.append(
            "game/src/lib.rs",
            r#"
struct Label;
impl Label { fn number(self) -> i32 { 3 } }
fn label(text: &'static str, log: &mut Vec<&'static str>) -> Label { log.push(text); Label }
pub fn spaced(log: &mut Vec<&'static str>) -> i32 {
    let first = label("two words", log).number();
    let second = label("twowords", log).number();
    first + second
}
"#,
        );
        f.append("driver/src/lib.rs", &format!("\n#[test]\nfn literal_receiver_selection() {{ let mut log=Vec::new(); assert_eq!(advanced_game::spaced(&mut log),6); assert_eq!(log,{expected:?}); }}\n"));
        let selector = format!("label(\"{selected}\",log).number");
        f.append("mods/alpha/horizon-mixin.toml", &format!("\n[[injection]]\nid=\"spaced\"\ntarget=\"advanced_game::spaced\"\nkind=\"BEFORE_CALL\"\nselector={}\nordinal=0\nrequire=1\nhelper=\"combined-before.rs\"\nbindings=[{{name=\"log\",ty=\"&mut Vec<&'static str>\",value=\"&mut *log\"}}]\n",serde_json::to_string(&selector).unwrap()));
        assert!(
            f.run("check")["cargo_stdout"]
                .as_str()
                .unwrap()
                .contains("9 passed")
        );
    }
}

#[test]
fn call_selector_identifier_boundaries_cannot_alias_a_cast() {
    let f = fixture();
    f.append("game/src/lib.rs", "\nstruct NumericLabel; impl NumericLabel { fn number(self) -> i32 { 3 } }\nfn numeric(_value: i64, _log: &mut Vec<&'static str>) -> NumericLabel { NumericLabel }\npub fn cast_selector(value: i32, log: &mut Vec<&'static str>) -> i32 { numeric(value as i64, log).number() }\n");
    f.append("mods/alpha/horizon-mixin.toml", "\n[[injection]]\nid=\"cast-selector\"\ntarget=\"advanced_game::cast_selector\"\nkind=\"BEFORE_CALL\"\nselector=\"numeric(valueasi64,log).number\"\nordinal=0\nrequire=1\nhelper=\"combined-before.rs\"\nbindings=[{name=\"log\",ty=\"&mut Vec<&'static str>\",value=\"&mut *log\"}]\n");
    failure(&f, "E_MATCH_COUNT");
}

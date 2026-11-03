use super::*;
fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("original");
    let mods = temp.path().join("mods");
    copy(&repo().join("fixtures/effects"), &root);
    copy(&repo().join("fixtures/effects-mods"), &mods);
    Fixture {
        out: temp.path().join("stage"),
        _temp: temp,
        root,
        mods,
    }
}
fn edit(
    f: &Fixture,
    section: &str,
    id: &str,
    change: impl FnOnce(&mut toml::map::Map<String, toml::Value>),
) {
    let path = f.mods.join("alpha/horizon-mixin.toml");
    let mut v: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    change(
        v[section]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|v| v["id"].as_str() == Some(id))
            .unwrap()
            .as_table_mut()
            .unwrap(),
    );
    fs::write(path, toml::to_string(&v).unwrap()).unwrap();
}
fn failure(f: &Fixture, code: &str) {
    let before = source::files(&f.root).unwrap();
    let error = build_plan(&f.root, &f.mods).unwrap_err();
    assert_eq!(error.code, code, "{error:?}");
    assert!(!error.message.is_empty());
    assert_eq!(source::files(&f.root).unwrap(), before);
    assert!(!f.out.exists());
}
#[test]
fn effects_compile_and_preserve_pristine_files_and_mtimes() {
    let f = fixture();
    let original = source::files(&f.root).unwrap();
    let times = mtimes(&f.root);
    let result = f.run("check");
    assert_eq!(result["states"], 2);
    assert!(
        result["cargo_stdout"]
            .as_str()
            .unwrap()
            .contains("5 passed")
    );
    let staged = source::files(&f.out).unwrap();
    let staged_times = mtimes(&f.out);
    assert_eq!(f.run("check")["stage_changed"], false);
    assert_eq!(source::files(&f.out).unwrap(), staged);
    assert_eq!(mtimes(&f.out), staged_times);
    assert_eq!(source::files(&f.root).unwrap(), original);
    assert_eq!(mtimes(&f.root), times);
}
#[test]
fn effects_snapshots_and_discovery_order_are_deterministic() {
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
    let mut portable = serde_json::to_value(&plan).unwrap();
    portable.as_object_mut().unwrap().remove("inputs");
    portable["rustc"] = "<pinned toolchain>".into();
    let json = serde_json::to_string_pretty(&portable).unwrap() + "\n";
    let snapshots = repo().join("crates/horizon-mixins-cli/tests/snapshots");
    for (name, bytes) in [
        ("effects.rs", output["game/src/lib.rs"].as_slice()),
        ("effects-plan.json", json.as_bytes()),
    ] {
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            fs::write(snapshots.join(name), bytes).unwrap();
        }
        assert_eq!(bytes, fs::read(snapshots.join(name)).unwrap(), "{name}");
    }
}
#[test]
fn expression_selection_is_exact_and_counts_original_sites() {
    let f = fixture();
    edit(&f, "injection", "expression", |v| {
        v.insert("require".into(), 2.into());
    });
    failure(&f, "E_MATCH_COUNT");
    let f = fixture();
    edit(&f, "injection", "expression", |v| {
        v.insert("ordinal".into(), 1.into());
    });
    failure(&f, "E_ORDINAL");
}
#[test]
fn expression_places_and_owned_reads_are_rejected() {
    let f = fixture();
    edit(&f, "injection", "expression", |v| {
        v.insert("selector".into(), "trace".into());
        v.insert("require".into(), 1.into());
        v["bindings"].as_array_mut().unwrap()[0]["ty"] = "Trace".into();
    });
    f.replace(
        "mods/alpha/expression.rs",
        "fn modify(value:i32,trace:&Trace)->i32",
        "fn modify(value:Trace,trace:&Trace)->Trace",
    );
    failure(&f, "E_UNSUPPORTED_SITE");
    let f = fixture();
    edit(&f, "injection", "expression", |v| {
        v.insert("selector".into(), "value".into());
    });
    // A scalar read is safe and compiles; replacing a place on an assignment is not.
    f.replace(
        "game/src/lib.rs",
        "let value = add(number(trace),3)*2; value",
        "let mut value = add(number(trace),3)*2; value=9; value",
    );
    edit(&f, "injection", "expression", |v| {
        v.insert("require".into(), 2.into());
    });
    failure(&f, "E_UNSUPPORTED_SITE");
}
#[test]
fn expression_and_call_contracts_must_agree() {
    let f = fixture();
    edit(&f, "injection", "nested", |v| {
        v.insert("target".into(), "effects_game::expression".into());
        v.insert("selector".into(), "add".into());
        v.insert("result".into(), "u32".into());
        v["bindings"].as_array_mut().unwrap()[0]["ty"] = "&mut u32".into();
    });
    f.replace("mods/alpha/nested.rs", "&mut i32", "&mut u32");
    failure(&f, "E_VALUE_SIGNATURE");
}
#[test]
fn after_call_requires_explicit_owned_result_and_correct_borrow() {
    let f = fixture();
    edit(&f, "injection", "nested", |v| {
        v.insert("result".into(), "&i32".into());
    });
    failure(&f, "E_AFTER_RESULT");
    let f = fixture();
    edit(&f, "injection", "nested", |v| {
        v.insert("result".into(), "u32".into());
    });
    failure(&f, "E_AFTER_RESULT");
}
#[test]
fn cancellation_and_assignment_reject_unproven_interior_sites() {
    let f = fixture();
    edit(&f, "injection", "before", |v| {
        v.insert("target".into(), "effects_game::nested".into());
    });
    failure(&f, "E_UNSUPPORTED_SITE");
    let f = fixture();
    edit(&f, "injection", "assigned-first", |v| {
        v.insert("target".into(), "effects_game::nested".into());
        v.insert("require".into(), 1.into());
    });
    failure(&f, "E_UNSUPPORTED_SITE");
    let f = fixture();
    f.replace(
        "game/src/lib.rs",
        "let mut value: i32=number(trace);",
        "let mut value: i32=number(trace)+1;",
    );
    failure(&f, "E_UNSUPPORTED_SITE");
}
#[test]
fn return_capture_must_exist_at_added_cancellation_exits() {
    let f = fixture();
    edit(&f, "injection", "return", |v| {
        v["bindings"]
            .as_array_mut()
            .unwrap()
            .push(toml::Value::Table(toml::map::Map::from_iter([
                ("name".into(), "local".into()),
                ("ty".into(), "&i32".into()),
                ("value".into(), "$local_ref:value:0".into()),
            ])));
    });
    f.replace(
        "mods/alpha/return.rs",
        "fn result(value:i32)->i32 {value+1}",
        "fn result(value:i32,local:&i32)->i32 {value+*local}",
    );
    failure(&f, "E_LOCAL_NOT_VISIBLE");
}
#[test]
fn state_requires_safe_layout_ownership_and_complete_constructor_coverage() {
    let f = fixture();
    f.replace(
        "game/src/lib.rs",
        "pub struct Entity",
        "#[repr(C)] pub struct Entity",
    );
    failure(&f, "E_STATE");
    let f = fixture();
    f.replace("game/src/lib.rs", "health:i32 }", "pub health:i32 }");
    failure(&f, "E_STATE");
    let f = fixture();
    edit(&f, "state", "history", |v| {
        v.insert("ty".into(), "&'static str".into());
    });
    failure(&f, "E_STATE");
    let f = fixture();
    edit(&f, "state", "history", |v| {
        v["constructor"].as_array_mut().unwrap().pop();
    });
    failure(&f, "E_STATE");
    let f = fixture();
    edit(&f, "state", "history", |v| {
        v["constructor"].as_array_mut().unwrap()[0]["require"] = 2.into();
    });
    failure(&f, "E_MATCH_COUNT");
}
#[test]
fn state_rejects_collisions_patterns_aliases_and_unproven_defaults() {
    let f = fixture();
    f.replace(
        "game/src/lib.rs",
        "health:i32 }",
        "health:i32, __horizon_mixins_alpha_history:Vec<i32> }",
    );
    failure(&f, "E_STATE_CONFLICT");
    let f = fixture();
    f.replace(
        "game/src/lib.rs",
        "pub fn health(&self)->i32 {self.health}",
        "pub fn health(&self)->i32 { let Self { health } = self; *health }",
    );
    failure(&f, "E_STATE");
    let f = fixture();
    edit(&f, "state", "history", |v| {
        v["constructor"].as_array_mut().unwrap()[0]["selector"] = "Alias".into();
    });
    failure(&f, "E_STATE");
    let f = fixture();
    f.replace(
        "game/src/lib.rs",
        "derive(Clone, Debug, Serialize, Deserialize)",
        "derive(Clone, Debug, Default, Serialize, Deserialize)",
    );
    failure(&f, "E_STATE");
}
#[test]
fn malformed_state_ir_fails_before_rewriting() {
    let f = fixture();
    let mut plan = build_plan(&f.root, &f.mods).unwrap();
    plan.states[0].constructors[0].node.clear();
    assert_eq!(
        horizon_mixins_transform::transform(&f.root, &plan)
            .unwrap_err()
            .code,
        "E_PLAN_SITE"
    );
    let mut plan = build_plan(&f.root, &f.mods).unwrap();
    plan.states[0].constructors[0].fingerprint = "changed".into();
    assert_eq!(
        horizon_mixins_transform::transform(&f.root, &plan)
            .unwrap_err()
            .code,
        "E_STALE_SOURCE"
    );
}
fn compiler_failure(f: &Fixture, expected: &str) {
    let before = source::files(&f.root).unwrap();
    let output = f.command("check").output().unwrap();
    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "E_BUILD", "{error}");
    assert!(
        error["message"].as_str().unwrap().contains(expected),
        "{error}"
    );
    assert_eq!(source::files(&f.root).unwrap(), before);
}
#[test]
fn new_contracts_remain_checked_by_rust() {
    let f = fixture();
    edit(&f, "injection", "boolean", |v| {
        v["bindings"].as_array_mut().unwrap()[0]["ty"] = "i32".into();
    });
    f.replace(
        "mods/alpha/boolean.rs",
        "fn modify(value:bool,trace:&Trace)->bool",
        "fn modify(value:i32,trace:&Trace)->i32",
    );
    compiler_failure(&f, "mismatched types");
    let f = fixture();
    edit(&f, "injection", "owned", |v| {
        v["bindings"].as_array_mut().unwrap()[0]["ty"] = "String".into();
        v["bindings"].as_array_mut().unwrap()[0]["value"] = "$local:text:0".into();
    });
    f.replace(
        "mods/alpha/owned.rs",
        "fn after(text:&mut String)",
        "fn after(mut text:String)",
    );
    compiler_failure(&f, "use of moved value");
    let f = fixture();
    edit(&f, "injection", "nested", |v| {
        v["bindings"]
            .as_array_mut()
            .unwrap()
            .push(toml::Value::Table(toml::map::Map::from_iter([
                ("name".into(), "second".into()),
                ("ty".into(), "&mut i32".into()),
                ("value".into(), "$result_mut".into()),
            ])));
    });
    f.replace(
        "mods/alpha/nested.rs",
        "fn after(value:&mut i32,trace:&Trace)",
        "fn after(value:&mut i32,trace:&Trace,second:&mut i32)",
    );
    compiler_failure(&f, "more than once at a time");
    let f = fixture();
    edit(&f, "state", "history", |v| {
        v.insert("initial".into(), "missing_initializer()".into());
    });
    compiler_failure(&f, "cannot find function");
}
#[test]
fn return_local_can_run_at_post_assignment_cancellation() {
    let f = fixture();
    f.append("mods/alpha/horizon-mixin.toml",r#"
[[injection]]
id="assigned-return"
target="effects_game::assigned"
kind="RETURN"
helper="assigned-return.rs"
bindings=[{name="value",ty="i32",value="$value"},{name="local",ty="&i32",value="$local_ref:value:0"}]
"#);
    fs::write(
        f.mods.join("alpha/assigned-return.rs"),
        "fn result(value:i32,local:&i32)->i32 {value+*local}",
    )
    .unwrap();
    f.replace(
        "driver/src/lib.rs",
        "assigned(&t,false),12",
        "assigned(&t,false),24",
    );
    f.replace(
        "driver/src/lib.rs",
        "assigned(&t,true),102",
        "assigned(&t,true),104",
    );
    f.run("check");
}
#[test]
fn owned_expression_results_and_non_cancellable_discard_preserve_drop_order() {
    let f = fixture();
    f.append(
        "mods/alpha/horizon-mixin.toml",
        r#"
[[injection]]
id="owned-value"
target="effects_game::after_owned"
kind="MODIFY_EXPRESSION_VALUE"
helper="owned-value.rs"
selector='String::from("hello")'
require=1
ordinal=0
bindings=[{name="value",ty="String",value="$value"}]
"#,
    );
    fs::write(
        f.mods.join("alpha/owned-value.rs"),
        "fn modify(mut value:String)->String {value.push_str(\"?\");value}",
    )
    .unwrap();
    f.replace(
        "driver/src/lib.rs",
        "after_owned(&t),\"hello!\"",
        "after_owned(&t),\"hello?!\"",
    );
    edit(&f, "injection", "after", |v| {
        v.insert("cancellable".into(), false.into());
    });
    fs::write(f.mods.join("alpha/after.rs"),"fn after(result:&Token,cancel:bool,trace:&Trace) {let _=(result,cancel);mark(trace,\"after-alpha\");}").unwrap();
    f.replace(
        "driver/src/lib.rs",
        "discarded(&t,true),41",
        "discarded(&t,true),5",
    );
    f.replace("driver/src/lib.rs","vec![\"receiver\",\"call\",\"after-alpha\",\"result-drop\",\"receiver-drop\"]","vec![\"receiver\",\"call\",\"after-alpha\",\"after-beta\",\"result-drop\",\"receiver-drop\",\"body\"]");
    f.append(
        "game/src/lib.rs",
        r#"
pub fn stored_drop(trace:&Trace) { let result=receiver(trace).produce(); mark(trace,"body"); }
pub fn borrowed()->&'static str {"original"}
pub fn stored_borrow()->usize {let mut value=borrowed(); value.len()}
"#,
    );
    f.append("mods/alpha/horizon-mixin.toml",r#"
[[injection]]
id="stored-drop"
target="effects_game::stored_drop"
kind="AFTER_ASSIGN"
selector="receiver(trace).produce"
require=1
ordinal=0
helper="stored-drop.rs"
bindings=[{name="result",ty="&Token",value="$local_ref:result:0"},{name="trace",ty="&Trace",value="trace"}]
[[injection]]
id="borrowed-value"
target="effects_game::stored_borrow"
kind="MODIFY_EXPRESSION_VALUE"
selector="borrowed()"
require=1
ordinal=0
helper="borrowed-value.rs"
bindings=[{name="value",ty="&'static str",value="$value"}]
[[injection]]
id="stored-borrow"
target="effects_game::stored_borrow"
kind="AFTER_ASSIGN"
selector="borrowed"
require=1
ordinal=0
helper="stored-borrow.rs"
bindings=[{name="value",ty="&mut &'static str",value="$local_mut:value:0"}]
"#);
    fs::write(
        f.mods.join("alpha/stored-drop.rs"),
        "fn after(result:&Token,trace:&Trace) {let _=result; mark(trace,\"stored\");}",
    )
    .unwrap();
    fs::write(
        f.mods.join("alpha/borrowed-value.rs"),
        "fn modify(value:&'static str)->&'static str {let _=value; \"modified\"}",
    )
    .unwrap();
    fs::write(
        f.mods.join("alpha/stored-borrow.rs"),
        "fn after(value:&mut &'static str) {assert_eq!(*value,\"modified\"); *value=\"abc\";}",
    )
    .unwrap();
    f.append("driver/src/lib.rs",r#"
#[test] fn stored_drop_and_borrow() {
    let trace=effects_game::Trace::default(); effects_game::stored_drop(&trace);
    assert_eq!(*trace.borrow(), vec!["receiver","call","receiver-drop","stored","body","result-drop"]);
    assert_eq!(effects_game::stored_borrow(),3);
}
"#);
    f.run("check");
}
#[test]
fn state_constructor_overwrite_and_namespaced_collisions_fail_in_planning() {
    let f = fixture();
    f.append(
        "mods/alpha/horizon-mixin.toml",
        r#"
[[injection]]
id="overwrite-new"
target="effects_game::state::Entity::new"
kind="OVERWRITE"
helper="overwrite-new.rs"
bindings=[{name="health",ty="i32",value="health"}]
"#,
    );
    fs::write(
        f.mods.join("alpha/overwrite-new.rs"),
        "fn replacement(health:i32)->Entity {Entity::new(health)}",
    )
    .unwrap();
    failure(&f, "E_STATE_CONFLICT");
    let f = fixture();
    f.append(
        "mods/alpha/horizon-mixin.toml",
        r#"
[[bridge]]
id="history"
kind="GET"
target="effects_game::state::Entity"
member="health"
"#,
    );
    failure(&f, "E_DUPLICATE_ID");
    let f = fixture();
    let path = f.mods.join("alpha/horizon-mixin.toml");
    let mut manifest: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let mut extra = manifest["state"].as_array().unwrap()[0].clone();
    extra["id"] = "history_mut".into();
    manifest["state"].as_array_mut().unwrap().push(extra);
    fs::write(path, toml::to_string(&manifest).unwrap()).unwrap();
    failure(&f, "E_STATE_CONFLICT");
}
#[test]
fn state_only_bundles_preserve_derived_default_and_drop_lifecycle() {
    let f = fixture();
    for bundle in ["alpha", "beta"] {
        let path = f.mods.join(bundle).join("horizon-mixin.toml");
        let mut manifest: toml::Value =
            toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        manifest.as_table_mut().unwrap().remove("injection");
        manifest["state"].as_array_mut().unwrap()[0]["initial"] =
            "::core::default::Default::default()".into();
        fs::write(path, toml::to_string(&manifest).unwrap()).unwrap();
    }
    f.replace(
        "game/src/lib.rs",
        "derive(Clone, Debug, Serialize, Deserialize)",
        "derive(Clone, Debug, Default, Serialize, Deserialize)",
    );
    f.append(
        "mods/alpha/horizon-mixin.toml",
        r#"
[[state]]
id="drop"
target="effects_game::state::Entity"
ty="DropState"
initial="::core::default::Default::default()"
[[state.constructor]]
target="effects_game::state::Entity::new"
selector="Self"
require=1
[[state.constructor]]
target="effects_game::state::Entity::with_health"
selector="Self"
require=1
"#,
    );
    f.replace(
        "game/src/lib.rs",
        "pub static INITIALIZATIONS",
        r#"
    #[derive(Clone, Debug, Default, Serialize, Deserialize)] pub struct DropState;
    pub static DROPS:AtomicUsize=AtomicUsize::new(0);
    impl Drop for DropState { fn drop(&mut self) {DROPS.fetch_add(1,Ordering::SeqCst);} }
    pub static INITIALIZATIONS"#,
    );
    fs::write(f.root.join("driver/src/lib.rs"),r#"
#[test] fn defaults_and_drop() {
    use effects_game::state::{Entity,DROPS}; use std::sync::atomic::Ordering;
    let mut a=Entity::default(); let b=Entity::new(5);
    a.__horizon_mixins_alpha_history_mut().push(1);
    assert!(b.__horizon_mixins_alpha_history().is_empty()); assert_eq!(*b.__horizon_mixins_beta_counter(),0);
    let before=DROPS.load(Ordering::SeqCst);
    let a=a.with_health(3); assert_eq!(DROPS.load(Ordering::SeqCst),before);
    drop(a); assert_eq!(DROPS.load(Ordering::SeqCst),before+1);
    drop(b); assert_eq!(DROPS.load(Ordering::SeqCst),before+2);
}
"#).unwrap();
    let result = f.run("check");
    assert_eq!(result["injections"], 0);
    assert_eq!(result["states"], 3);
}

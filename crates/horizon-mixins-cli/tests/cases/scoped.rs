use super::*;
fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("original");
    let mods = temp.path().join("mods");
    copy(&repo().join("fixtures/scoped"), &root);
    copy(&repo().join("fixtures/scoped-mods"), &mods);
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
    section: &str,
    id: &str,
    change: impl FnOnce(&mut toml::map::Map<String, toml::Value>),
) {
    let path = f.mods.join(bundle).join("horizon-mixin.toml");
    let mut manifest: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let entry = manifest[section]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|i| i["id"].as_str() == Some(id))
        .unwrap();
    change(entry.as_table_mut().unwrap());
    fs::write(path, toml::to_string(&manifest).unwrap()).unwrap();
}
fn capture(f: &Fixture, bundle: &str, id: &str, value: &str) {
    edit(f, bundle, "injection", id, |i| {
        i["bindings"].as_array_mut().unwrap()[0]["value"] = value.into();
    });
}
fn failure(f: &Fixture, code: &str) {
    let originals = source::files(&f.root).unwrap();
    let error = build_plan(&f.root, &f.mods).unwrap_err();
    assert_eq!(error.code, code, "{error:?}");
    assert!(!error.message.is_empty());
    assert!(error.file.is_some(), "{error:?}");
    assert_eq!(source::files(&f.root).unwrap(), originals);
    assert!(!f.out.exists());
}
fn compiler_failure(f: &Fixture, message: &str) {
    let originals = source::files(&f.root).unwrap();
    let output = f.command("check").output().unwrap();
    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "E_BUILD");
    assert!(
        error["message"].as_str().unwrap().contains(message),
        "{error}"
    );
    assert_eq!(source::files(&f.root).unwrap(), originals);
}
#[test]
fn scoped_features_compile_preserve_inputs_and_unchanged_mtimes() {
    let f = fixture();
    let original = source::files(&f.root).unwrap();
    let original_times = mtimes(&f.root);
    let report = f.run("check");
    assert_eq!(report["bridges"], 9);
    assert_eq!(report["injections"], 25);
    assert!(
        report["cargo_stdout"]
            .as_str()
            .unwrap()
            .contains("8 passed")
    );
    let first = source::files(&f.out).unwrap();
    let times = mtimes(&f.out);
    assert_eq!(f.run("check")["stage_changed"], false);
    assert_eq!(source::files(&f.out).unwrap(), first);
    assert_eq!(mtimes(&f.out), times);
    assert_eq!(source::files(&f.root).unwrap(), original);
    assert_eq!(mtimes(&f.root), original_times);
    f.replace("mods/alpha/store.rs", "value+3", "value+3+0");
    assert_eq!(f.run("check")["stage_changed"], true);
    assert_eq!(
        mtimes(&f.out)["game/src/state.rs"],
        times["game/src/state.rs"]
    );
}
#[test]
fn scoped_source_plan_and_bridge_snapshots_ignore_discovery_order() {
    let f = fixture();
    let plan = build_plan(&f.root, &f.mods).unwrap();
    assert_eq!(plan.schema, 7);
    assert_eq!(plan.bridges.len(), 9);
    let captures = &plan
        .injections
        .iter()
        .find(|p| p.mod_id == "alpha" && p.injection_id == "capture")
        .unwrap()
        .captures;
    assert_eq!(captures[0].local.name, "value");
    assert_eq!(captures[0].local.ordinal, 0);
    assert!(!captures[0].local.parameter);
    fs::rename(f.mods.join("alpha"), f.mods.join("z-alpha")).unwrap();
    fs::rename(f.mods.join("beta"), f.mods.join("a-beta")).unwrap();
    assert_eq!(
        serde_json::to_vec(&plan).unwrap(),
        serde_json::to_vec(&build_plan(&f.root, &f.mods).unwrap()).unwrap()
    );
    let output = horizon_mixins_transform::transform(&f.root, &plan).unwrap();
    let mut portable = serde_json::to_value(&plan).unwrap();
    portable.as_object_mut().unwrap().remove("inputs");
    portable.as_object_mut().unwrap().remove("rustc");
    let snapshots = repo().join("crates/horizon-mixins-cli/tests/snapshots");
    for (name, bytes) in [
        ("scoped.rs", output["game/src/lib.rs"].clone()),
        ("bridges.rs", output["game/src/state.rs"].clone()),
        (
            "scoped-plan.json",
            (serde_json::to_string_pretty(&portable).unwrap() + "\n").into_bytes(),
        ),
    ] {
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            fs::write(snapshots.join(name), &bytes).unwrap();
        }
        assert_eq!(bytes, fs::read(snapshots.join(name)).unwrap(), "{name}");
    }
}
#[test]
fn captures_reject_missing_shadowed_out_of_scope_and_unsupported_bindings() {
    for (bundle, id, value, code) in [
        (
            "alpha",
            "capture",
            "$local_mut:missing:0",
            "E_LOCAL_NOT_FOUND",
        ),
        (
            "alpha",
            "capture",
            "$local_mut:value:9",
            "E_LOCAL_NOT_FOUND",
        ),
        ("alpha", "capture", "$local_mut:value", "E_INVALID_CAPTURE"),
        ("beta", "inner", "$local_ref:value:0", "E_LOCAL_NOT_VISIBLE"),
        ("beta", "inner", "$local_mut:value:1", "E_INVALID_CAPTURE"),
    ] {
        let f = fixture();
        capture(&f, bundle, id, value);
        failure(&f, code);
    }
    for replacement in [
        "if let Some(value)=Some(100){tick(log);let _=value;}",
        "match Some(100){Some(value)=>{tick(log);},_=>{}}",
    ] {
        let f = fixture();
        f.replace(
            "game/src/lib.rs",
            "{let value:i32=100;tick(log);let _=value;}",
            replacement,
        );
        failure(&f, "E_UNSUPPORTED_LOCAL");
    }
    let f = fixture();
    f.replace(
        "game/src/lib.rs",
        "{let value:i32=100;tick(log);let _=value;}",
        "{let value:i32=100;let _=value;}{tick(log);}",
    );
    failure(&f, "E_LOCAL_NOT_VISIBLE");
    let f = fixture();
    f.replace(
        "game/src/lib.rs",
        "let extra:i32=3;\n    if early{return 5;}",
        "if early{return 5;}\n    let extra:i32=3;",
    );
    failure(&f, "E_LOCAL_NOT_VISIBLE");
    let f = fixture();
    f.replace("mods/alpha/capture.rs", "value:&mut i32", "value:&mut i64");
    edit(&f, "alpha", "injection", "capture", |i| {
        i["bindings"].as_array_mut().unwrap()[0]["ty"] = "&mut i64".into();
    });
    failure(&f, "E_CAPTURE_TYPE");
    let f = fixture();
    fs::write(
        f.mods.join("alpha/init.rs"),
        "fn modify(value:i32,log:&mut Vec<&'static str>,old:&i32)->i32{let _=(log,old);value}",
    )
    .unwrap();
    edit(&f, "alpha", "injection", "init", |i| {
        i["bindings"].as_array_mut().unwrap().push(
            toml::from_str::<toml::Value>("name='old'\nty='&i32'\nvalue='$local_ref:health:0'")
                .unwrap(),
        );
    });
    failure(&f, "E_LOCAL_NOT_VISIBLE");
}
#[test]
fn return_captures_cannot_run_before_their_declarations() {
    let f = fixture();
    fs::write(
        f.mods.join("alpha/cancel.rs"),
        "fn cancel()->core::ops::ControlFlow<i32>{core::ops::ControlFlow::Break(1)}",
    )
    .unwrap();
    f.append("mods/alpha/horizon-mixin.toml","\n[[injection]]\nid='cancel'\nkind='HEAD'\ncancellable=true\ntarget='scoped_game::return_local'\nhelper='cancel.rs'\n");
    failure(&f, "E_LOCAL_NOT_VISIBLE");
}
#[test]
fn captured_values_and_references_remain_borrow_checked() {
    let f = fixture();
    fs::write(
        f.mods.join("alpha/inferred.rs"),
        "fn capture(value:&mut String){value.push('x');}",
    )
    .unwrap();
    edit(&f, "alpha", "injection", "inferred", |i| {
        i["bindings"].as_array_mut().unwrap()[0]["ty"] = "&mut String".into();
    });
    compiler_failure(&f, "mismatched types");
    let f = fixture();
    f.append("game/src/lib.rs","\npub fn owned_capture(log:&mut Vec<&'static str>)->usize{let text=String::from(\"x\");tick(log);text.len()}\n");
    fs::write(
        f.mods.join("alpha/take.rs"),
        "fn take(text:String){drop(text);}",
    )
    .unwrap();
    f.append("mods/alpha/horizon-mixin.toml","\n[[injection]]\nid='take-owned'\nkind='BEFORE_CALL'\ntarget='scoped_game::owned_capture'\nselector='tick'\nrequire=1\nordinal=0\nhelper='take.rs'\nbindings=[{name='text',ty='String',value='$local:text:0'}]\n");
    compiler_failure(&f, "moved value");
    let f = fixture();
    fs::write(f.mods.join("alpha/capture.rs"),"fn capture(value:&mut i32,log:&mut Vec<&'static str>,again:&mut i32){*value+=1;*again+=1;log.push(\"capture\");}").unwrap();
    edit(&f, "alpha", "injection", "capture", |i| {
        i["bindings"].as_array_mut().unwrap().push(
            toml::from_str::<toml::Value>(
                "name='again'\nty='&mut i32'\nvalue='$local_mut:value:0'",
            )
            .unwrap(),
        );
    });
    compiler_failure(&f, "more than once");
}
#[test]
fn slices_validate_global_bounds_scope_order_and_filtered_counts() {
    for (part, field, value, code) in [
        ("from", "require", toml::Value::Integer(2), "E_SLICE_COUNT"),
        ("from", "require", toml::Value::Integer(0), "E_SLICE"),
        (
            "from",
            "ordinal",
            toml::Value::Integer(1),
            "E_SLICE_ORDINAL",
        ),
        (
            "from",
            "selector",
            toml::Value::String("missing".into()),
            "E_SLICE_COUNT",
        ),
        (
            "from",
            "selector",
            toml::Value::String("end".into()),
            "E_SLICE_ORDER",
        ),
    ] {
        let f = fixture();
        edit(&f, "alpha", "injection", "slice-arg", |i| {
            i["slice"][part][field] = value;
        });
        failure(&f, code);
    }
    let f = fixture();
    edit(&f, "alpha", "injection", "slice-arg", |i| {
        i["slice"] = toml::Value::Table(Default::default());
    });
    failure(&f, "E_SLICE");
    let f = fixture();
    edit(&f, "alpha", "injection", "slice-arg", |i| {
        i["require"] = 3.into();
    });
    failure(&f, "E_MATCH_COUNT");
    let f = fixture();
    f.replace("game/src/lib.rs", "    begin(log);", "    {begin(log);}");
    failure(&f, "E_SLICE_SCOPE");
    let f = fixture();
    edit(&f, "alpha", "injection", "return-local", |i| {
        i.insert("slice".into(), toml::Value::Table(Default::default()));
    });
    failure(&f, "E_SLICE");
}
#[test]
fn bridge_targets_ids_ownership_and_reserved_names_are_validated() {
    for (section, id, key, value, code) in [
        (
            "bridge",
            "get-health",
            "member",
            "missing",
            "E_BRIDGE_TARGET",
        ),
        (
            "bridge",
            "get-health",
            "target",
            "scoped_game::state::Missing",
            "E_BRIDGE_TARGET",
        ),
        (
            "bridge",
            "inc",
            "target",
            "scoped_game::locals",
            "E_BRIDGE_TARGET",
        ),
        ("bridge", "get-health", "id", "capture", "E_DUPLICATE_ID"),
    ] {
        let f = fixture();
        edit(&f, "alpha", section, id, |i| {
            i.insert(key.into(), value.into());
        });
        failure(&f, code);
    }
    let f = fixture();
    edit(&f, "beta", "bridge", "tuple-field", |i| {
        i.insert("member".into(), "9".into());
    });
    failure(&f, "E_BRIDGE_TARGET");
    let f = fixture();
    f.append(
        "game/src/state.rs",
        "\nimpl Entity{pub fn __horizon_mixins_alpha_get_health(&self)->&i32{&self.health}}\n",
    );
    failure(&f, "E_BRIDGE_CONFLICT");
    let f = fixture();
    f.append("game/src/state.rs", "\npub struct Generic<T>{value:T}\n");
    edit(&f, "alpha", "bridge", "get-health", |i| {
        i["target"] = "scoped_game::state::Generic".into();
        i["member"] = "value".into();
    });
    failure(&f, "E_BRIDGE_TARGET");
    for (from, to, code) in [
        (
            "pub struct Entity",
            "#[cfg(feature=\"uncertain\")] pub struct Entity",
            "E_UNSUPPORTED_CFG",
        ),
        (
            "health:i32,name:String",
            "#[cfg(feature=\"uncertain\")] health:i32,name:String",
            "E_UNSUPPORTED_CFG",
        ),
        ("fn inc(", "async fn inc(", "E_BRIDGE_TARGET"),
    ] {
        let f = fixture();
        f.replace("game/src/state.rs", from, to);
        failure(&f, code);
    }
    let f = fixture();
    fs::create_dir(f.mods.join("alias-a")).unwrap();
    fs::create_dir(f.mods.join("alias-b")).unwrap();
    for (folder, id) in [("alias-a", "a-b"), ("alias-b", "a_b")] {
        fs::write(f.mods.join(folder).join("horizon-mixin.toml"),format!("[mod]\nid='{id}'\nversion='1'\n[[bridge]]\nid='get'\nkind='GET'\ntarget='scoped_game::state::Entity'\nmember='health'\n")).unwrap();
    }
    failure(&f, "E_BRIDGE_CONFLICT");
}
#[test]
fn public_bridges_do_not_expose_private_fields_or_allow_aliasing_mutable_borrows() {
    let f = fixture();
    f.append(
        "driver/src/lib.rs",
        "\npub fn illegal(entity:&scoped_game::state::Entity)->i32{entity.health}\n",
    );
    compiler_failure(&f, "private");
    let f = fixture();
    f.append("driver/src/lib.rs","\npub fn aliases(entity:&mut scoped_game::state::Entity){let a=entity.__horizon_mixins_alpha_get_health_mut();let b=entity.__horizon_mixins_alpha_get_health_mut();*a+=*b;}\n");
    compiler_failure(&f, "more than once");
}
#[test]
fn tuple_contracts_and_cross_kind_conflicts_fail_before_staging() {
    let f = fixture();
    edit(&f, "alpha", "injection", "tuple", |i| {
        i.remove("result");
    });
    failure(&f, "E_ARGUMENT_SIGNATURE");
    let f = fixture();
    edit(&f, "alpha", "injection", "tuple", |i| {
        i["result"] = "&str".into();
    });
    failure(&f, "E_ARGUMENT_SIGNATURE");
    let f = fixture();
    edit(&f, "alpha", "injection", "tuple", |i| {
        i["bindings"].as_array_mut().unwrap()[0]["ty"] = "(String,i32)".into();
    });
    failure(&f, "E_ARGUMENT_SIGNATURE");
    let f = fixture();
    f.replace("mods/beta/tuple.rs", "i32", "i64");
    edit(&f, "beta", "injection", "tuple", |i| {
        i["bindings"].as_array_mut().unwrap()[0]["ty"] = "(String,i64,Trace)".into();
    });
    failure(&f, "E_ARGUMENT_SIGNATURE");
    let f = fixture();
    f.replace("mods/alpha/single.rs", "i32", "i64");
    edit(&f, "alpha", "injection", "single", |i| {
        i["bindings"].as_array_mut().unwrap()[0]["ty"] = "i64".into();
    });
    failure(&f, "E_ARGUMENT_SIGNATURE");
    let f = fixture();
    f.replace(
        "game/src/lib.rs",
        "entity.add_traced(next(evaluations,&trace),trace)",
        "entity.add_traced(*entity.__horizon_mixins_alpha_get_health(),trace)",
    );
    failure(&f, "E_UNSUPPORTED_SITE");
    let f = fixture();
    f.append("mods/alpha/horizon-mixin.toml","\n[[injection]]\nid='overwrite-tuple'\nkind='OVERWRITE'\ntarget='scoped_game::jointly'\nhelper='overwrite.rs'\n");
    fs::write(
        f.mods.join("alpha/overwrite.rs"),
        "fn replacement()->String{String::new()}",
    )
    .unwrap();
    failure(&f, "E_OVERWRITE_CONFLICT");
}
#[test]
fn variable_selection_rejects_unsupported_accesses_and_declaration_types() {
    let f = fixture();
    edit(&f, "alpha", "injection", "store", |i| {
        i["variable"]["ordinal"] = 9.into();
    });
    failure(&f, "E_LOCAL_NOT_FOUND");
    let f = fixture();
    edit(&f, "alpha", "injection", "store", |i| {
        i["require"] = 3.into();
    });
    failure(&f, "E_MATCH_COUNT");
    let f = fixture();
    edit(&f, "alpha", "injection", "store", |i| {
        i["ordinal"] = 9.into();
    });
    failure(&f, "E_ORDINAL");
    let f = fixture();
    f.replace("game/src/lib.rs", "health=health+2;", "health+=2;");
    failure(&f, "E_UNSUPPORTED_LOCAL");
    let f = fixture();
    f.replace("mods/alpha/store.rs", "i32", "i64");
    edit(&f, "alpha", "injection", "store", |i| {
        i["bindings"].as_array_mut().unwrap()[0]["ty"] = "i64".into();
    });
    failure(&f, "E_VARIABLE_TYPE");
    let f = fixture();
    edit(&f, "alpha", "injection", "store", |i| {
        i.insert("selector".into(), "health".into());
    });
    failure(&f, "E_SELECTOR");
}
#[test]
fn malformed_scoped_plans_return_structured_errors() {
    let f = fixture();
    let plan = build_plan(&f.root, &f.mods).unwrap();
    let mut bad = plan.clone();
    bad.bridges[0].node.clear();
    assert_eq!(
        horizon_mixins_transform::transform(&f.root, &bad)
            .unwrap_err()
            .code,
        "E_PLAN_SITE"
    );
    let mut bad = plan.clone();
    bad.bridges[0].fingerprint = "wrong".into();
    assert_eq!(
        horizon_mixins_transform::transform(&f.root, &bad)
            .unwrap_err()
            .code,
        "E_STALE_SOURCE"
    );
    let mut bad = plan.clone();
    bad.injections
        .iter_mut()
        .find(|p| p.variable_site.is_some())
        .unwrap()
        .variable_site = None;
    assert_eq!(
        horizon_mixins_transform::transform(&f.root, &bad)
            .unwrap_err()
            .code,
        "E_PLAN_SITE"
    );
}

#[test]
fn bridges_are_reusable_from_injection_helpers_and_handle_macro_method_bodies() {
    let f = fixture();
    f.replace(
        "game/src/state.rs",
        "self.health+=amount;",
        "assert!(amount>=0);self.health+=amount;",
    );
    f.append("game/src/lib.rs","\npub fn through_bridge(entity:&mut Entity)->i32{*entity.__horizon_mixins_alpha_get_health()}\n");
    fs::write(
        f.mods.join("alpha/through.rs"),
        "fn initialize(entity:&mut Entity){entity.__horizon_mixins_alpha_set_health(42);}",
    )
    .unwrap();
    f.append("mods/alpha/horizon-mixin.toml","\n[[injection]]\nid='through'\nkind='HEAD'\ntarget='scoped_game::through_bridge'\nhelper='through.rs'\nbindings=[{name='entity',ty='&mut Entity',value='&mut *entity'}]\n");
    f.append("driver/src/lib.rs","\n#[test]fn reusable_bridges(){let mut e=scoped_game::state::Entity::new();assert_eq!(scoped_game::through_bridge(&mut e),42);}\n");
    assert!(
        f.run("check")["cargo_stdout"]
            .as_str()
            .unwrap()
            .contains("9 passed")
    );
    // A bridge-only mod needs no dummy injection and can compile independently.
    let f = fixture();
    for bundle in ["alpha", "beta"] {
        let path = f.mods.join(bundle).join("horizon-mixin.toml");
        let mut m: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        m.as_table_mut().unwrap().remove("injection");
        fs::write(path, toml::to_string(&m).unwrap()).unwrap();
    }
    let plan = build_plan(&f.root, &f.mods).unwrap();
    assert!(plan.injections.is_empty());
    assert_eq!(plan.bridges.len(), 9);
    assert_eq!(
        success(
            f.command("check")
                .args(["--package", "scoped-game"])
                .output()
                .unwrap()
        )["bridges"],
        9
    );
}
#[test]
fn open_slices_select_relative_ordinals_and_exclude_end_boundaries() {
    for (side, ordinal, expected) in [("from", 1, [2, 4, 26]), ("to", 0, [22, 4, 6])] {
        let f = fixture();
        f.append("game/src/lib.rs","\npub fn open(log:&mut Vec<&'static str>)->[i32;3]{let a=consume(1);begin(log);let b=consume(2);end(log);let c=consume(3);[a,b,c]}\n");
        let boundary = if side == "from" { "begin" } else { "end" };
        f.append("mods/alpha/horizon-mixin.toml",&format!("\n[[injection]]\nid='open'\nkind='MODIFY_ARG'\ntarget='scoped_game::open'\nselector='consume'\nrequire=2\nordinal={ordinal}\nargument=0\nhelper='slice-arg.rs'\nbindings=[{{name='value',ty='i32',value='$value'}}]\nslice={{ {side}={{selector='{boundary}',require=1,ordinal=0}} }}\n"));
        f.append("driver/src/lib.rs",&format!("\n#[test]fn open_boundary(){{let mut log=Vec::new();assert_eq!(scoped_game::open(&mut log),{expected:?});}}\n"));
        assert!(
            f.run("check")["cargo_stdout"]
                .as_str()
                .unwrap()
                .contains("9 passed")
        );
    }
}
#[test]
fn variable_identity_and_load_before_store_order_survive_shadowing() {
    let f = fixture();
    f.append("game/src/lib.rs","\npub fn shadow_variables()->(i32,i32){let mut x:i32=1;x=2;let inner={let mut x:i32=3;x=4;x};(x,inner)}\npub fn variable_order()->i32{let x:i32=2;let y:i32=x;y}\n");
    for (id, target, name, local, at, require, ordinal, expr, priority) in [
        (
            "outer-store",
            "shadow_variables",
            "x",
            0,
            "STORE",
            2,
            1,
            "value*10",
            100,
        ),
        (
            "inner-store",
            "shadow_variables",
            "x",
            1,
            "STORE",
            2,
            1,
            "value+1",
            100,
        ),
        (
            "order-load",
            "variable_order",
            "x",
            0,
            "LOAD",
            1,
            0,
            "value*3",
            0,
        ),
        (
            "order-store",
            "variable_order",
            "y",
            0,
            "STORE",
            1,
            0,
            "value+1",
            100,
        ),
    ] {
        fs::write(
            f.mods.join("alpha").join(format!("{id}.rs")),
            format!("fn modify(value:i32)->i32{{{expr}}}"),
        )
        .unwrap();
        f.append("mods/alpha/horizon-mixin.toml",&format!("\n[[injection]]\nid='{id}'\nkind='MODIFY_VARIABLE'\ntarget='scoped_game::{target}'\nhelper='{id}.rs'\npriority={priority}\nvariable={{name='{name}',ordinal={local},at='{at}'}}\nrequire={require}\nordinal={ordinal}\nbindings=[{{name='value',ty='i32',value='$value'}}]\n"));
    }
    f.append("driver/src/lib.rs","\n#[test]fn variable_scopes(){assert_eq!(scoped_game::shadow_variables(),(20,5));assert_eq!(scoped_game::variable_order(),7);}\n");
    assert!(
        f.run("check")["cargo_stdout"]
            .as_str()
            .unwrap()
            .contains("9 passed")
    );
}
#[test]
fn owned_stores_preserve_assignment_destruction_order() {
    let f = fixture();
    f.append("game/src/lib.rs",r#"
struct Stored{trace:Trace,name:&'static str}
impl Drop for Stored{fn drop(&mut self){self.trace.borrow_mut().push(self.name);}}
pub fn owned_store(trace:Trace){let mut value:Stored=Stored{trace:trace.clone(),name:"old"};value=Stored{trace,name:"new"};drop(value);}
"#);
    fs::write(
        f.mods.join("alpha/owned-store.rs"),
        "fn modify(value:Stored)->Stored{value.trace.borrow_mut().push(\"store\");value}",
    )
    .unwrap();
    f.append("mods/alpha/horizon-mixin.toml","\n[[injection]]\nid='owned-store'\nkind='MODIFY_VARIABLE'\ntarget='scoped_game::owned_store'\nvariable={name='value',ordinal=0,at='STORE'}\nrequire=2\nordinal=1\nhelper='owned-store.rs'\nbindings=[{name='value',ty='Stored',value='$value'}]\n");
    f.append("driver/src/lib.rs","\n#[test]fn owned_store_drop(){let trace=std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));scoped_game::owned_store(trace.clone());assert_eq!(*trace.borrow(),[\"store\",\"old\",\"new\"]);}\n");
    assert!(
        f.run("check")["cargo_stdout"]
            .as_str()
            .unwrap()
            .contains("9 passed")
    );
}
#[test]
fn variable_loads_do_not_rewrite_places_or_owned_implicit_borrows() {
    for (source, ty) in [
        (
            "pub fn place()->i32{let value:i32=1;let borrowed=&value;*borrowed}",
            "i32",
        ),
        (
            "pub fn place()->String{let value:String=String::from(\"x\");value}",
            "String",
        ),
    ] {
        let f = fixture();
        f.append("game/src/lib.rs", &format!("\n{source}\n"));
        fs::write(
            f.mods.join("alpha/place.rs"),
            format!("fn modify(value:{ty})->{ty}{{value}}"),
        )
        .unwrap();
        f.append("mods/alpha/horizon-mixin.toml",&format!("\n[[injection]]\nid='place'\nkind='MODIFY_VARIABLE'\ntarget='scoped_game::place'\nvariable={{name='value',ordinal=0,at='LOAD'}}\nrequire=1\nordinal=0\nhelper='place.rs'\nbindings=[{{name='value',ty='{ty}',value='$value'}}]\n"));
        failure(&f, "E_UNSUPPORTED_LOCAL");
    }
}

#[test]
fn bridge_metadata_tracks_versions_and_rejects_unresolved_type_projections() {
    let f = fixture();
    let first = build_plan(&f.root, &f.mods).unwrap();
    f.replace(
        "mods/alpha/horizon-mixin.toml",
        "version = \"0.1.0\"",
        "version = \"0.2.0\"",
    );
    let second = build_plan(&f.root, &f.mods).unwrap();
    let a = first.bridges.iter().find(|b| b.mod_id == "alpha").unwrap();
    let b = second.bridges.iter().find(|b| b.mod_id == "alpha").unwrap();
    assert_ne!(a.mod_version, b.mod_version);
    assert_eq!(a.name, b.name);
    assert_eq!(a.generated, b.generated);
    let f = fixture();
    f.append("game/src/state.rs","\npub trait Associated{type Out;fn assoc(&self)->Self::Out;}impl Associated for Entity{type Out=i32;fn assoc(&self)->Self::Out{self.health}}\n");
    f.append("mods/alpha/horizon-mixin.toml","\n[[bridge]]\nid='associated'\nkind='INVOKE'\ntarget='scoped_game::state::<Entity as Associated>::assoc'\n");
    failure(&f, "E_BRIDGE_TARGET");
}

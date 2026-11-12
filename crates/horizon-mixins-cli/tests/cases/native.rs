use super::*;
fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("original");
    let mods = temp.path().join("mods");
    copy(&repo().join("fixtures/native"), &root);
    copy(&repo().join("fixtures/native-mods"), &mods);
    Fixture {
        out: temp.path().join("stage"),
        _temp: temp,
        root,
        mods,
    }
}
fn manifest(f: &Fixture, mod_dir: &str, change: impl FnOnce(&mut toml::Table)) {
    let path = f.mods.join(mod_dir).join("horizon-mixin.toml");
    let mut v: toml::Table = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    change(&mut v);
    fs::write(path, toml::to_string(&v).unwrap()).unwrap();
}
fn first<'a>(v: &'a mut toml::Table, section: &str) -> &'a mut toml::Table {
    v.get_mut(section).unwrap().as_array_mut().unwrap()[0]
        .as_table_mut()
        .unwrap()
}
fn failure(f: &Fixture, code: &str) -> horizon_mixins_core::Diagnostic {
    let before = source::files(&f.root).unwrap();
    let error = build_plan(&f.root, &f.mods).unwrap_err();
    assert_eq!(error.code, code, "{error:?}");
    assert!(!error.message.is_empty());
    assert_eq!(source::files(&f.root).unwrap(), before);
    assert!(!f.out.exists());
    error
}
fn engine_manifest(f: &Fixture, text: &str) {
    fs::write(f.mods.join("boost/engine/Cargo.toml"), text).unwrap();
}
const ENGINE: &str = "[package]\nname = \"boost-engine\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n";

#[test]
fn bundle_crate_data_and_named_hook_compile_with_the_locked_registry() {
    let f = fixture();
    let original = source::files(&f.root).unwrap();
    let original_times = mtimes(&f.root);
    let result = f.run("check");
    assert_eq!(result["crates"], 1);
    assert_eq!(result["data"], 1);
    assert_eq!(result["post_build"], 1);
    let stdout = result["cargo_stdout"].as_str().unwrap();
    // The driver's test drives the hook; the bundle crate's own unit test runs as a member.
    assert!(stdout.contains("named_hook_calls_the_bundle_crate_and_reads_bundle_data ... ok"));
    assert!(stdout.contains("bonus_comes_from_the_crate_data ... ok"));
    let staged = source::files(&f.out).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&staged["Cargo.toml"]),
        "[workspace]\n# The bundle crates are appended to this list in the stage.\nmembers = [\n    \"game\",\n    \"driver\",\n    \"native-crates/boost/engine\",\n]\nresolver = \"2\"\n"
    );
    assert!(
        String::from_utf8_lossy(&staged["game/Cargo.toml"]).ends_with(
            "serde = { version = \"1\", features = [\"derive\"] }\nboost-engine = { path = \"../native-crates/boost/engine\" }\n"
        )
    );
    let lock = String::from_utf8_lossy(&staged["Cargo.lock"]);
    assert!(lock.contains(
        "[[package]]\nname = \"boost-engine\"\nversion = \"0.1.0\"\ndependencies = [\n \"serde_json\",\n]\n\n[[package]]\nname = \"equivalent\""
    ));
    assert!(lock.contains("name = \"native-game\"\nversion = \"0.1.0\"\ndependencies = [\n \"boost-engine\",\n \"serde\",\n]"));
    assert_eq!(
        staged["native-crates/boost/engine/data/table.json"],
        b"{\"bonus\": 40}\n"
    );
    assert_eq!(
        staged["native-data/boost/nested/blob.bin"],
        [0x00, 0xff, 0x10, 0x80]
    );
    assert_eq!(
        String::from_utf8_lossy(&staged["native-post-build.json"]),
        "[\n  {\n    \"mod\": \"boost\",\n    \"package\": \"boost-engine\",\n    \"bin\": \"bake\",\n    \"args\": [\n      \"--quick\"\n    ]\n  }\n]\n"
    );
    // The plan records the resolved target and the hook it came from.
    let plan = build_plan(&f.root, &f.mods).unwrap();
    assert_eq!(plan.schema, 6);
    let tick = &plan.injections[0];
    assert_eq!(tick.target, "native_game::hooks::Game::native_tick");
    assert_eq!(
        (tick.hook.as_deref(), tick.hook_version),
        (Some("tick"), Some(1))
    );
    assert_eq!(plan.crates[0].lock_dependencies, ["serde_json"]);
    // Rerunning keeps every byte and mtime; originals are untouched.
    let times = mtimes(&f.out);
    assert_eq!(f.run("check")["stage_changed"], false);
    assert_eq!(source::files(&f.out).unwrap(), staged);
    assert_eq!(mtimes(&f.out), times);
    assert_eq!(source::files(&f.root).unwrap(), original);
    assert_eq!(mtimes(&f.root), original_times);
    // Editing the helper republishes the stage but leaves the crate's and data's files alone,
    // so Cargo's fingerprints for the bundle crate stay fresh.
    f.replace("mods/boost/tick.rs", ".trim()", ".trim_end()");
    assert_eq!(f.run("apply")["stage_changed"], true);
    let new_times = mtimes(&f.out);
    for (name, time) in &times {
        if name != "game/src/lib.rs" && name != ".horizon-mixins-stage.json" {
            assert_eq!(new_times[name], *time, "mtime changed for {name}");
        }
    }
}
#[test]
fn mods_without_crates_still_get_an_empty_post_build_list() {
    let f = fixture();
    manifest(&f, "boost", |v| {
        v.remove("crate");
        v.remove("post_build");
    });
    fs::remove_dir_all(f.mods.join("boost/engine")).unwrap();
    f.replace(
        "mods/boost/tick.rs",
        "game.score = boost_engine::boost(game.score);",
        "game.score += 40;",
    );
    let result = f.run("check");
    assert_eq!(result["crates"], 0);
    let staged = source::files(&f.out).unwrap();
    assert_eq!(staged["native-post-build.json"], b"[]\n");
    assert_eq!(
        staged["Cargo.lock"],
        fs::read(f.root.join("Cargo.lock")).unwrap()
    );
    assert_eq!(
        staged["Cargo.toml"],
        fs::read(f.root.join("Cargo.toml")).unwrap()
    );
    assert!(staged.contains_key("native-data/boost/greeting.txt"));
}
#[test]
fn named_hook_catalog_errors_list_what_exists() {
    let f = fixture();
    fs::remove_file(f.root.join("native-hooks.json")).unwrap();
    // Cargo.lock and the sources are unchanged, so only the catalog is missing.
    let e = failure(&f, "E_HOOK_CATALOG");
    assert_eq!(e.injection_id.as_deref(), Some("tick"));
    let f = fixture();
    fs::write(f.root.join("native-hooks.json"), "{\"hooks\": [").unwrap();
    failure(&f, "E_HOOK_CATALOG");
    let f = fixture();
    fs::write(
        f.root.join("native-hooks.json"),
        r#"{"hooks":[{"name":"tick","version":1,"target":"a::b"},{"name":"tick","version":1,"target":"a::c"}]}"#,
    )
    .unwrap();
    failure(&f, "E_HOOK_CATALOG");
    let f = fixture();
    manifest(&f, "boost", |v| {
        first(v, "injection").insert("hook".into(), "tock".into());
    });
    let e = failure(&f, "E_HOOK_UNKNOWN");
    assert!(e.message.contains("known hooks: frame, tick"), "{e:?}");
    let f = fixture();
    manifest(&f, "boost", |v| {
        first(v, "injection").insert("hook_version".into(), 2.into());
    });
    let e = failure(&f, "E_HOOK_VERSION");
    assert!(e.message.contains("has version 1"), "{e:?}");
    // A hook that resolves to a target the index lacks fails like a raw target would.
    let f = fixture();
    manifest(&f, "boost", |v| {
        first(v, "injection").insert("hook".into(), "frame".into());
    });
    fs::write(
        f.root.join("native-hooks.json"),
        r#"{"hooks":[{"name":"frame","version":1,"target":"native_game::hooks::Game::gone"}]}"#,
    )
    .unwrap();
    assert_ne!(
        build_plan(&f.root, &f.mods).unwrap_err().code,
        "E_HOOK_UNKNOWN"
    );
}
#[test]
fn hook_and_target_are_exclusive() {
    let f = fixture();
    manifest(&f, "boost", |v| {
        first(v, "injection").insert(
            "target".into(),
            "native_game::hooks::Game::native_tick".into(),
        );
    });
    failure(&f, "E_MANIFEST");
    let f = fixture();
    manifest(&f, "boost", |v| {
        first(v, "injection").remove("hook_version");
    });
    failure(&f, "E_MANIFEST");
    let f = fixture();
    manifest(&f, "boost", |v| {
        first(v, "injection").remove("hook");
    });
    failure(&f, "E_MANIFEST");
}
#[test]
fn bundle_crates_must_stay_inside_horizons_workspace_and_lock() {
    for (text, code) in [
        (
            format!("{ENGINE}serde_json = \"1\"\n\n[workspace]\n"),
            "E_CRATE",
        ),
        (
            format!("{ENGINE}local = {{ path = \"../x\" }}\n"),
            "E_CRATE_DEPENDENCY",
        ),
        (
            format!("{ENGINE}remote = {{ git = \"https://example.com/x\" }}\n"),
            "E_CRATE_DEPENDENCY",
        ),
        (
            format!("{ENGINE}serde_json = {{ version = \"1\", workspace = true }}\n"),
            "E_CRATE_DEPENDENCY",
        ),
        (format!("{ENGINE}rand = \"0.8\"\n"), "E_CRATE_DEPENDENCY"),
        (
            format!("{ENGINE}serde_json = \"2\"\n"),
            "E_CRATE_DEPENDENCY",
        ),
        (
            format!("{ENGINE}\n[target.'cfg(unix)'.dependencies]\nserde_json = \"1\"\n"),
            "E_CRATE",
        ),
        (
            format!("{ENGINE}\n[lib]\npath = \"../../tick.rs\"\n"),
            "E_CRATE",
        ),
        (ENGINE.replace("boost-engine", "native-game"), "E_CRATE"),
        (ENGINE.replace("boost-engine", "serde_json"), "E_CRATE"),
        (
            ENGINE.replace("version = \"0.1.0\"", "version.workspace = true"),
            "E_CRATE",
        ),
    ] {
        let f = fixture();
        engine_manifest(&f, &text);
        let e = failure(&f, code);
        assert_eq!(e.mod_id.as_deref(), Some("boost"), "{text}");
        if text.contains("rand") {
            assert_eq!(
                &*e.message,
                "boost needs rand ^0.8, which isn't in Horizon's Cargo.lock"
            );
        }
    }
    let f = fixture();
    fs::write(f.mods.join("boost/engine/Cargo.lock"), "version = 4\n").unwrap();
    failure(&f, "E_CRATE");
    let f = fixture();
    manifest(&f, "boost", |v| {
        first(v, "crate").insert("dependent".into(), "horizon-client".into());
    });
    let e = failure(&f, "E_CRATE");
    assert!(e.message.contains("native-driver, native-game"), "{e:?}");
    let f = fixture();
    manifest(&f, "boost", |v| {
        first(v, "crate").insert("path".into(), "missing".into());
    });
    failure(&f, "E_IO");
    let f = fixture();
    manifest(&f, "boost", |v| {
        first(v, "crate").insert("path".into(), "data/inner".into());
    });
    failure(&f, "E_CRATE");
    let f = fixture();
    manifest(&f, "boost", |v| {
        first(v, "crate").insert("path".into(), "../boost/engine".into());
    });
    failure(&f, "E_INVALID_PATH");
    // The dependent already depends on a package of that name.
    let f = fixture();
    f.append("game/Cargo.toml", "boost_engine = \"1\"\n");
    failure(&f, "E_CRATE");
}
#[test]
fn two_mods_cannot_ship_the_same_package() {
    let f = fixture();
    copy(&f.mods.join("boost"), &f.mods.join("second"));
    manifest(&f, "second", |v| {
        v["mod"].as_table_mut().unwrap()["id"] = "second".into();
        v.remove("post_build");
    });
    let e = failure(&f, "E_CRATE");
    assert!(e.message.contains("mod 'boost' crate 'engine'"), "{e:?}");
    assert_eq!(e.mod_id.as_deref(), Some("second"));
    // Two mods ship different packages: both become members, in mod id order.
    let f = fixture();
    copy(&f.mods.join("boost"), &f.mods.join("second"));
    manifest(&f, "second", |v| {
        v["mod"].as_table_mut().unwrap()["id"] = "second".into();
        v.remove("post_build");
        v.remove("injection");
    });
    fs::remove_file(f.mods.join("second/tick.rs")).unwrap();
    fs::remove_dir_all(f.mods.join("second/engine/src/bin")).unwrap();
    fs::write(
        f.mods.join("second/engine/Cargo.toml"),
        ENGINE.replace("boost-engine", "second-engine") + "serde_json = \"1\"\n",
    )
    .unwrap();
    // The second package sorts after serde_json in Cargo.lock; --locked accepts both entries.
    let staged = f.run("check");
    assert_eq!(staged["crates"], 2);
    let lock = fs::read_to_string(f.out.join("Cargo.lock")).unwrap();
    assert!(
        lock.contains("dependencies = [\n \"boost-engine\",\n \"second-engine\",\n \"serde\",\n]")
    );
}
#[test]
fn post_build_steps_name_a_bin_of_the_mods_own_crate() {
    for (key, value) in [("crate", "other"), ("bin", "../bake"), ("bin", "missing")] {
        let f = fixture();
        manifest(&f, "boost", |v| {
            first(v, "post_build").insert(key.into(), value.into());
        });
        let e = failure(&f, "E_POST_BUILD");
        if value == "missing" {
            assert!(e.message.contains("it has: bake"), "{e:?}");
        }
    }
    // A [[bin]] table names a bin too.
    let f = fixture();
    fs::rename(
        f.mods.join("boost/engine/src/bin/bake.rs"),
        f.mods.join("boost/engine/src/bake_main.rs"),
    )
    .unwrap();
    fs::write(
        f.mods.join("boost/engine/Cargo.toml"),
        format!(
            "{ENGINE}serde_json = \"1\"\n\n[[bin]]\nname = \"bake\"\npath = \"src/bake_main.rs\"\n"
        ),
    )
    .unwrap();
    assert_eq!(build_plan(&f.root, &f.mods).unwrap().post_build.len(), 1);
}
#[test]
fn a_bin_table_that_claims_an_inferred_file_replaces_the_inferred_bin() {
    // Cargo drops the bin it would infer from a file that a [[bin]] table claims by path.
    for (from, to, claimed, inferred) in [
        (
            "src/bin/bake.rs",
            "src/main.rs",
            "src/main.rs",
            "boost-engine",
        ),
        (
            "src/bin/bake.rs",
            "src/bin/other.rs",
            "src/bin/other.rs",
            "other",
        ),
    ] {
        let f = fixture();
        let engine = f.mods.join("boost/engine");
        fs::create_dir_all(engine.join(to).parent().unwrap()).unwrap();
        fs::rename(engine.join(from), engine.join(to)).unwrap();
        let cargo = format!(
            "{ENGINE}serde_json = \"1\"\n\n[[bin]]\nname = \"bake\"\npath = \"{claimed}\"\n"
        );
        fs::write(engine.join("Cargo.toml"), &cargo).unwrap();
        assert_eq!(build_plan(&f.root, &f.mods).unwrap().post_build.len(), 1);
        manifest(&f, "boost", |v| {
            first(v, "post_build").insert("bin".into(), inferred.into());
        });
        let e = failure(&f, "E_POST_BUILD");
        assert!(e.message.contains("it has: bake"), "{e:?}");
    }
}
#[test]
fn post_build_bins_never_share_a_file_in_the_target_folder() {
    // Two mods' steps both named "bake": one would run the other's program.
    let f = fixture();
    copy(&f.mods.join("boost"), &f.mods.join("second"));
    manifest(&f, "second", |v| {
        v["mod"].as_table_mut().unwrap()["id"] = "second".into();
        v.remove("injection");
    });
    fs::remove_file(f.mods.join("second/tick.rs")).unwrap();
    fs::write(
        f.mods.join("second/engine/Cargo.toml"),
        ENGINE.replace("boost-engine", "second-engine") + "serde_json = \"1\"\n",
    )
    .unwrap();
    fs::rename(
        f.mods.join("second/engine/src/bin/bake.rs"),
        f.mods.join("second/engine/src/bin/Bake.rs"),
    )
    .unwrap();
    manifest(&f, "second", |v| {
        first(v, "post_build").insert("bin".into(), "Bake".into());
    });
    let e = failure(&f, "E_POST_BUILD");
    assert!(
        e.message.contains("of mod 'boost' (crate 'engine')"),
        "{e:?}"
    );
    assert_eq!(e.mod_id.as_deref(), Some("second"));
    // A step named like one of the workspace's bins would replace that program.
    let f = fixture();
    fs::write(f.root.join("driver/src/main.rs"), "fn main() {}\n").unwrap();
    fs::rename(
        f.mods.join("boost/engine/src/bin/bake.rs"),
        f.mods.join("boost/engine/src/bin/native-driver.rs"),
    )
    .unwrap();
    manifest(&f, "boost", |v| {
        first(v, "post_build").insert("bin".into(), "native-driver".into());
    });
    let e = failure(&f, "E_POST_BUILD");
    assert!(
        e.message.contains("the workspace's bin 'native-driver'"),
        "{e:?}"
    );
    // One mod may run the same bin twice, with other arguments.
    let f = fixture();
    manifest(&f, "boost", |v| {
        let steps = v["post_build"].as_array_mut().unwrap();
        steps.push(steps[0].clone());
    });
    assert_eq!(build_plan(&f.root, &f.mods).unwrap().post_build.len(), 2);
}
#[test]
fn default_features_may_link_locked_packages_but_never_add_one() {
    // hashbrown 0.15 is locked without its default features (the driver's, as in Horizon), while
    // allocator-api2 and equivalent, which the defaults use, are locked for other crates.
    let f = fixture();
    engine_manifest(
        &f,
        &format!("{ENGINE}serde_json = \"1\"\nhashbrown = \"0.15\"\n"),
    );
    let first = f.run("check");
    assert_eq!(first["crates"], 1);
    let lock = fs::read_to_string(f.out.join("Cargo.lock")).unwrap();
    let hashbrown = lock
        .split("\n\n")
        .find(|b| b.contains("name = \"hashbrown\""))
        .unwrap();
    assert!(
        hashbrown.ends_with(
            "dependencies = [\n \"allocator-api2\",\n \"equivalent\",\n \"foldhash\",\n]"
        ),
        "{hashbrown}"
    );
    let original = fs::read_to_string(f.root.join("Cargo.lock")).unwrap();
    let packages = |text: &str| text.matches("[[package]]").count();
    assert_eq!(
        packages(&lock),
        packages(&original) + 1,
        "only boost-engine is added"
    );
    let times = mtimes(&f.out);
    assert_eq!(f.run("apply")["stage_changed"], false);
    assert_eq!(mtimes(&f.out), times);
    // serde_json's preserve_order needs indexmap, which the lock lacks: refused, nothing staged.
    let f = fixture();
    engine_manifest(
        &f,
        &format!("{ENGINE}serde_json = {{ version = \"1\", features = [\"preserve_order\"] }}\n"),
    );
    let output = f.command("apply").output().unwrap();
    assert!(!output.status.success());
    let text = String::from_utf8_lossy(&output.stdout) + String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("E_CRATE_DEPENDENCY"), "{text}");
    assert!(
        text.contains("boost needs ") && text.contains("indexmap 2."),
        "{text}"
    );
    assert!(!f.out.exists());
}
#[cfg(unix)]
#[test]
fn symlinks_and_reserved_stage_paths_are_rejected() {
    use std::os::unix::fs::symlink;
    let f = fixture();
    symlink("/etc/hostname", f.mods.join("boost/data/link.txt")).unwrap();
    failure(&f, "E_INVALID_PATH");
    let f = fixture();
    let data = f.mods.join("boost/data");
    fs::rename(&data, f.mods.join("real-data")).unwrap();
    symlink(f.mods.join("real-data"), &data).unwrap();
    failure(&f, "E_INVALID_PATH");
    let f = fixture();
    symlink("../tick.rs", f.mods.join("boost/engine/src/linked.rs")).unwrap();
    failure(&f, "E_INVALID_PATH");
    let f = fixture();
    let engine = f.mods.join("boost/engine");
    fs::rename(&engine, f.mods.join("real-engine")).unwrap();
    symlink(f.mods.join("real-engine"), &engine).unwrap();
    failure(&f, "E_INVALID_PATH");
    let f = fixture();
    fs::create_dir_all(f.root.join("native-data")).unwrap();
    fs::write(f.root.join("native-data/x"), "").unwrap();
    failure(&f, "E_INVALID_PATH");
}
#[test]
fn crates_need_a_bundle_directory() {
    let f = fixture();
    fs::write(
        f.mods.join("loose.toml"),
        "[mod]\nid = \"loose\"\nversion = \"1\"\n\n[[crate]]\nid = \"c\"\npath = \"boost/engine\"\n",
    )
    .unwrap();
    failure(&f, "E_CRATE");
}
#[test]
fn a_crlf_checkout_stages_and_builds() {
    // Git for Windows' core.autocrlf checks the workspace out with CRLF.
    let f = fixture();
    for name in [
        "Cargo.lock",
        "Cargo.toml",
        "game/Cargo.toml",
        "driver/Cargo.toml",
        "native-hooks.json",
    ] {
        let path = f.root.join(name);
        let text = fs::read_to_string(&path).unwrap();
        fs::write(path, text.replace('\n', "\r\n")).unwrap();
    }
    assert_eq!(f.run("check")["crates"], 1);
    let lock = fs::read_to_string(f.out.join("Cargo.lock")).unwrap();
    assert!(lock.contains("name = \"boost-engine\"\r\n"), "{lock}");
    assert!(
        !lock.replace("\r\n", "").contains('\n'),
        "the lock keeps CRLF"
    );
}

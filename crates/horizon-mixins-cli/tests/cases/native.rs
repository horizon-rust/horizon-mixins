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
const SIBLING_ENGINE_MANIFEST: &str = "[package]\nname = \"boost-engine\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nengine_math = { path = \"../../shared/math\", package = \"utility-kernel\" }\n";

fn sibling_fixture_with(engine_manifest_text: &str) -> Fixture {
    let f = fixture();
    manifest(&f, "boost", |v| {
        let declarations: toml::Value = toml::from_str(
            "[[crate]]\nid = \"engine\"\npath = \"libraries/apps/engine\"\ndependent = \"native-game\"\n\n[[crate]]\nid = \"math\"\npath = \"libraries/shared/math\"\ndependent = \"native-game\"\n",
        )
        .unwrap();
        v.insert("crate".into(), declarations["crate"].clone());
    });
    let engine = f.mods.join("boost/libraries/apps/engine");
    fs::create_dir_all(engine.parent().unwrap()).unwrap();
    fs::rename(f.mods.join("boost/engine"), &engine).unwrap();
    let math = f.mods.join("boost/libraries/shared/math");
    fs::create_dir_all(math.join("src")).unwrap();
    fs::write(engine.join("Cargo.toml"), engine_manifest_text).unwrap();
    fs::write(
        engine.join("src/lib.rs"),
        "pub fn boost(score: i64) -> i64 { score + engine_math::bonus() }\n",
    )
    .unwrap();
    fs::write(
        math.join("Cargo.toml"),
        "[package]\nname = \"utility-kernel\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(math.join("src/lib.rs"), "pub fn bonus() -> i64 { 40 }\n").unwrap();
    f
}

fn sibling_fixture() -> Fixture {
    sibling_fixture_with(SIBLING_ENGINE_MANIFEST)
}

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
    assert_eq!(plan.schema, 7);
    assert!(
        serde_json::to_value(&plan).unwrap()["crates"][0]
            .get("dependencies")
            .is_none()
    );
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
fn loose_toml_mod_without_crates_plans_alongside_bundle_crate() {
    let f = fixture();
    fs::write(
        f.mods.join("loose.toml"),
        "[mod]\nid = \"loose\"\nversion = \"0.1.0\"\n\n[[injection]]\nid = \"frame\"\nhook = \"frame\"\nhook_version = 1\nkind = \"HEAD\"\nhelper = \"loose.rs\"\nbindings = [{ name = \"game\", ty = \"&mut Game\", value = \"self\" }]\n",
    )
    .unwrap();
    fs::write(
        f.mods.join("loose.rs"),
        "fn frame(game: &mut Game) { game.score += 1; }\n",
    )
    .unwrap();

    let plan = build_plan(&f.root, &f.mods).unwrap();

    assert_eq!(plan.crates.len(), 1);
    assert!(plan.injections.iter().any(|injection| {
        injection.mod_id == "loose"
            && injection.injection_id == "frame"
            && injection.target == "native_game::hooks::Game::native_frame"
    }));
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
fn sibling_crate_dependencies_compile_forward_reference_nested_roots_and_package_alias() {
    let f = fixture();
    manifest(&f, "boost", |v| {
        let declarations: toml::Value = toml::from_str(
            r#"
[[crate]]
id = "engine"
path = "libraries/apps/engine"
dependent = "native-game"

[[crate]]
id = "math"
path = "libraries/shared/math"
dependent = "native-game"
"#,
        )
        .unwrap();
        v.insert("crate".into(), declarations["crate"].clone());
    });

    let engine_root = f.mods.join("boost/libraries/apps/engine");
    fs::create_dir_all(engine_root.parent().unwrap()).unwrap();
    fs::rename(f.mods.join("boost/engine"), &engine_root).unwrap();
    let math_root = f.mods.join("boost/libraries/shared/math");
    fs::create_dir_all(math_root.join("src")).unwrap();
    fs::write(
        engine_root.join("Cargo.toml"),
        "[package]\nname = \"boost-engine\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nengine_math = { path = \"../../shared/math\", package = \"utility-kernel\" }\n",
    )
    .unwrap();
    fs::write(
        engine_root.join("src/lib.rs"),
        "pub fn boost(score: i64) -> i64 { score + engine_math::bonus() }\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn engine_uses_the_sibling_library_value() {\n        assert_eq!(super::boost(2), 42);\n    }\n}\n",
    )
    .unwrap();
    fs::write(
        math_root.join("Cargo.toml"),
        "[package]\nname = \"utility-kernel\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(
        math_root.join("src/lib.rs"),
        "pub fn bonus() -> i64 { 40 }\n",
    )
    .unwrap();

    // The engine is declared before its dependency. Both roots are nested in the bundle, and
    // engine_math (the Cargo alias) differs from both the target crate ID and package name.
    let original_workspace = source::files(&f.root).unwrap();
    let original_workspace_times = mtimes(&f.root);
    let original_mods = source::files(&f.mods).unwrap();
    let original_mod_times = mtimes(&f.mods);
    let result = f.run("check");
    assert_eq!(result["crates"], 2);
    let stdout = result["cargo_stdout"].as_str().unwrap();
    assert!(
        stdout.contains("named_hook_calls_the_bundle_crate_and_reads_bundle_data ... ok"),
        "{stdout}"
    );
    assert!(
        stdout.contains("engine_uses_the_sibling_library_value ... ok"),
        "{stdout}"
    );
    let plan = build_plan(&f.root, &f.mods).unwrap();
    assert_eq!(plan.schema, 7);
    assert_eq!(plan.crates[0].dependencies.len(), 1);
    let edge = &plan.crates[0].dependencies[0];
    assert_eq!(
        (
            edge.table.as_str(),
            edge.key.as_str(),
            edge.target_crate_id.as_str()
        ),
        ("dependencies", "engine_math", "math")
    );
    let serialized = serde_json::to_value(&plan).unwrap();
    let serialized_edge = &serialized["crates"][0]["dependencies"][0];
    assert_eq!(serialized_edge.as_object().unwrap().len(), 3);

    let staged = source::files(&f.out).unwrap();
    let root_manifest = String::from_utf8_lossy(&staged["Cargo.toml"]);
    assert!(root_manifest.contains("native-crates/boost/engine"));
    assert!(root_manifest.contains("native-crates/boost/math"));
    assert!(staged.contains_key("native-crates/boost/engine/src/lib.rs"));
    assert_eq!(
        staged["native-crates/boost/math/src/lib.rs"],
        b"pub fn bonus() -> i64 { 40 }\n"
    );
    let staged_engine_manifest =
        String::from_utf8_lossy(&staged["native-crates/boost/engine/Cargo.toml"]);
    assert!(
        staged_engine_manifest
            .contains("engine_math = { path = \"../math\", package = \"utility-kernel\" }"),
        "{staged_engine_manifest}"
    );
    let game_manifest = String::from_utf8_lossy(&staged["game/Cargo.toml"]);
    assert!(
        game_manifest.contains("boost-engine = { path = \"../native-crates/boost/engine\" }"),
        "{game_manifest}"
    );

    let original_lock: toml::Value =
        toml::from_str(&String::from_utf8_lossy(&original_workspace["Cargo.lock"])).unwrap();
    let staged_lock: toml::Value =
        toml::from_str(&String::from_utf8_lossy(&staged["Cargo.lock"])).unwrap();
    fn package<'a>(lock: &'a toml::Value, name: &str) -> &'a toml::Value {
        lock["package"]
            .as_array()
            .unwrap()
            .iter()
            .find(|package| package["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("missing Cargo.lock package {name}"))
    }
    let engine_dependencies: Vec<_> = package(&staged_lock, "boost-engine")["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|dependency| dependency.as_str().unwrap())
        .collect();
    assert_eq!(engine_dependencies, ["utility-kernel"]);
    assert_eq!(
        package(&staged_lock, "utility-kernel")["version"].as_str(),
        Some("0.1.0")
    );
    let registry_packages = |lock: &toml::Value| {
        lock["package"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|package| {
                package
                    .get("source")
                    .and_then(toml::Value::as_str)
                    .is_some_and(|source| source.starts_with("registry+"))
            })
            .map(|package| {
                (
                    package["name"].as_str().unwrap().to_owned(),
                    package["version"].as_str().unwrap().to_owned(),
                    package["source"].as_str().unwrap().to_owned(),
                    package["checksum"].as_str().unwrap().to_owned(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        registry_packages(&staged_lock),
        registry_packages(&original_lock)
    );

    let stage_times = mtimes(&f.out);
    assert_eq!(f.run("apply")["stage_changed"], false);
    assert_eq!(source::files(&f.out).unwrap(), staged);
    assert_eq!(mtimes(&f.out), stage_times);
    assert_eq!(source::files(&f.root).unwrap(), original_workspace);
    assert_eq!(mtimes(&f.root), original_workspace_times);
    assert_eq!(source::files(&f.mods).unwrap(), original_mods);
    assert_eq!(mtimes(&f.mods), original_mod_times);
}

#[test]
fn sibling_optional_normal_build_and_dev_dependencies_compile_when_used() {
    let f = sibling_fixture();
    manifest(&f, "boost", |v| {
        let declarations: toml::Value = toml::from_str(
            r#"
[[crate]]
id = "builder"
path = "libraries/shared/builder"
dependent = "native-game"

[[crate]]
id = "tester"
path = "libraries/tools/tester"
dependent = "native-game"
"#,
        )
        .unwrap();
        for declaration in declarations["crate"].as_array().unwrap() {
            v["crate"].as_array_mut().unwrap().push(declaration.clone());
        }
    });

    let engine = f.mods.join("boost/libraries/apps/engine");
    fs::write(
        engine.join("Cargo.toml"),
        r#"[package]
name = "boost-engine"
version = "0.1.0"
edition = "2024"

[features]
default = ["engine_math"]

[dependencies]
engine_math = { path = "../../shared/math", package = "utility-kernel", optional = true }

[build-dependencies]
build_oracle = { path = "../../shared/builder", package = "build-oracle" }

[dev-dependencies]
test_oracle = { path = "../../tools/tester", package = "test-oracle" }
"#,
    )
    .unwrap();
    fs::write(
        engine.join("src/lib.rs"),
        r#"pub fn boost(score: i64) -> i64 { score + engine_math::bonus() }

#[cfg(test)]
mod tests {
    #[test]
    fn engine_uses_the_optional_sibling_feature() {
        assert_eq!(super::boost(2), 42);
    }

    #[test]
    fn engine_uses_the_dev_sibling_dependency() {
        assert_eq!(test_oracle::test_value(), 17);
    }
}
"#,
    )
    .unwrap();
    fs::write(
        engine.join("build.rs"),
        "fn main() { assert_eq!(build_oracle::build_value(), 7); }\n",
    )
    .unwrap();

    let builder = f.mods.join("boost/libraries/shared/builder");
    fs::create_dir_all(builder.join("src")).unwrap();
    fs::write(
        builder.join("Cargo.toml"),
        "[package]\nname = \"build-oracle\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(
        builder.join("src/lib.rs"),
        "pub fn build_value() -> i32 { 7 }\n",
    )
    .unwrap();

    let tester = f.mods.join("boost/libraries/tools/tester");
    fs::create_dir_all(tester.join("src")).unwrap();
    fs::write(
        tester.join("Cargo.toml"),
        "[package]\nname = \"test-oracle\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(
        tester.join("src/lib.rs"),
        "pub fn test_value() -> i32 { 17 }\n",
    )
    .unwrap();

    let result = f.run("check");
    assert_eq!(result["crates"], 4);
    let stdout = result["cargo_stdout"].as_str().unwrap();
    assert!(
        stdout.contains("engine_uses_the_optional_sibling_feature ... ok"),
        "{stdout}"
    );
    assert!(
        stdout.contains("engine_uses_the_dev_sibling_dependency ... ok"),
        "{stdout}"
    );
    let staged = source::files(&f.out).unwrap();
    let engine_manifest = String::from_utf8_lossy(&staged["native-crates/boost/engine/Cargo.toml"]);
    assert!(
        engine_manifest
            .contains("path = \"../math\", package = \"utility-kernel\", optional = true")
    );
    assert!(engine_manifest.contains("path = \"../builder\", package = \"build-oracle\""));
    assert!(engine_manifest.contains("path = \"../tester\", package = \"test-oracle\""));
}

#[test]
fn sibling_paths_reject_escapes_missing_undeclared_self_and_nonportable_paths() {
    for path in [
        "",
        "/absolute/math",
        "C:/absolute/math",
        r"\rooted\math",
        r"\\server\share\math",
        r"..\shared\math",
        "../../../../outside",
        "../../shared/missing",
        "../engine",
    ] {
        let engine_manifest = format!(
            "[package]\nname = \"boost-engine\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nengine_math = {{ path = {path:?}, package = \"utility-kernel\" }}\n"
        );
        let f = sibling_fixture_with(&engine_manifest);
        let error = failure(&f, "E_CRATE_DEPENDENCY");
        assert!(
            error.message.contains("[dependencies] engine_math"),
            "{error:?}"
        );
    }

    let engine_manifest =
        SIBLING_ENGINE_MANIFEST.replace("../../shared/math", "../../shared/other");
    let f = sibling_fixture_with(&engine_manifest);
    let other = f.mods.join("boost/libraries/shared/other");
    fs::create_dir_all(&other).unwrap();
    fs::write(
        other.join("Cargo.toml"),
        "[package]\nname = \"other-kernel\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    let error = failure(&f, "E_CRATE_DEPENDENCY");
    assert!(
        error.message.contains("another declared crate root"),
        "{error:?}"
    );
}

#[test]
fn sibling_alias_package_version_and_path_table_types_are_validated() {
    let matching = SIBLING_ENGINE_MANIFEST.replace(
        "package = \"utility-kernel\"",
        "package = \"utility-kernel\", version = \"0.1\"",
    );
    let f = sibling_fixture_with(&matching);
    let plan = build_plan(&f.root, &f.mods).unwrap();
    assert_eq!(plan.crates[0].dependencies.len(), 1);

    for (dependency, detail) in [
        (
            "{ path = \"../../shared/math\" }",
            "names package 'engine_math'",
        ),
        (
            "{ path = \"../../shared/math\", package = \"wrong-package\" }",
            "names package 'wrong-package'",
        ),
        (
            "{ path = \"../../shared/math\", package = \"utility-kernel\", version = \"0.2.0\" }",
            "does not match version requirement",
        ),
        (
            "{ path = false, package = \"utility-kernel\" }",
            "'path' must be a string",
        ),
        (
            "{ path = \"../../shared/math\", package = 7 }",
            "'package' must be a string",
        ),
        (
            "{ path = \"../../shared/math\", package = \"utility-kernel\", version = 7 }",
            "'version' must be a string",
        ),
        (
            "{ path = \"../../shared/math\", package = \"utility-kernel\", features = \"serde\" }",
            "'features' must be an array of strings",
        ),
        (
            "{ path = \"../../shared/math\", package = \"utility-kernel\", features = [true] }",
            "'features' must be an array of strings",
        ),
        (
            "{ path = \"../../shared/math\", package = \"utility-kernel\", default-features = \"false\" }",
            "'default-features' must be a boolean",
        ),
        (
            "{ path = \"../../shared/math\", package = \"utility-kernel\", optional = 1 }",
            "'optional' must be a boolean",
        ),
        (
            "{ path = \"../../shared/math\", package = \"utility-kernel\", git = \"https://example.invalid/math\" }",
            "'git' is not allowed",
        ),
    ] {
        let engine_manifest = format!(
            "[package]\nname = \"boost-engine\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nengine_math = {dependency}\n"
        );
        let f = sibling_fixture_with(&engine_manifest);
        let error = failure(&f, "E_CRATE_DEPENDENCY");
        assert!(
            error.message.contains(detail),
            "expected {detail:?}: {error:?}"
        );
    }
}

#[test]
fn sibling_normal_build_dev_and_optional_edges_are_in_the_plan_and_lock() {
    let engine_manifest = "[package]\nname = \"boost-engine\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nnormal_math = { path = \"../../shared/math\", package = \"utility-kernel\", optional = true }\n\n[build-dependencies]\nbuild_math = { path = \"../../shared/math\", package = \"utility-kernel\" }\n\n[dev-dependencies]\ndev_math = { path = \"../../shared/math\", package = \"utility-kernel\" }\n";
    let f = sibling_fixture_with(engine_manifest);
    let plan = build_plan(&f.root, &f.mods).unwrap();
    let engine = &plan.crates[0];
    assert_eq!(engine.dependencies.len(), 3);
    assert_eq!(
        engine
            .dependencies
            .iter()
            .map(|edge| edge.table.as_str())
            .collect::<Vec<_>>(),
        ["dependencies", "dev-dependencies", "build-dependencies"]
    );
    assert_eq!(engine.lock_dependencies, ["utility-kernel"]);
}

#[test]
fn sibling_staging_rewrites_each_dependency_table_and_keeps_options_and_comments() {
    let engine_manifest = "[package]\nname = \"boost-engine\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n# Keep this alias available for the bundle API.\nengine_math = { path = \"../../shared/math\", package = \"utility-kernel\", version = \"0.1.0\", features = [\"math-api\"], default-features = false, optional = true } # retained inline note\n\n[dev-dependencies]\nmath_test = { path = \"../../shared/math\", package = \"utility-kernel\", optional = true }\n\n[build-dependencies]\nmath_build = { path = \"../../shared/math\", package = \"utility-kernel\" }\n";
    let f = sibling_fixture_with(engine_manifest);
    let (plan, extras) = horizon_mixins_core::build_staged(&f.root, &f.mods).unwrap();
    let mut files = source::files(&f.root).unwrap();
    horizon_mixins_core::native::stage(&plan, &mut files, extras).unwrap();
    let staged = String::from_utf8_lossy(&files["native-crates/boost/engine/Cargo.toml"]);
    assert_eq!(staged.matches("path = \"../math\"").count(), 3, "{staged}");
    assert!(staged.contains("engine_math = {"), "{staged}");
    assert!(staged.contains("math_test = {"), "{staged}");
    assert!(staged.contains("math_build = {"), "{staged}");
    assert!(staged.contains("package = \"utility-kernel\""), "{staged}");
    assert!(staged.contains("version = \"0.1.0\""), "{staged}");
    assert!(staged.contains("features = [\"math-api\"]"), "{staged}");
    assert!(staged.contains("default-features = false"), "{staged}");
    assert!(staged.contains("optional = true"), "{staged}");
    assert!(
        staged.contains("# Keep this alias available for the bundle API."),
        "{staged}"
    );
    assert!(staged.contains("# retained inline note"), "{staged}");
}

#[test]
fn duplicate_physical_sibling_roots_are_rejected() {
    let f = sibling_fixture();
    manifest(&f, "boost", |v| {
        v["crate"].as_array_mut().unwrap()[1]
            .as_table_mut()
            .unwrap()
            .insert("path".into(), "libraries/apps/engine".into());
    });
    let error = failure(&f, "E_INVALID_PATH");
    assert!(
        error.message.contains("same physical source root"),
        "{error:?}"
    );
}

#[test]
fn sibling_direct_optional_dev_and_transitive_cycles_are_rejected_deterministically() {
    let f = sibling_fixture();
    fs::write(
        f.mods.join("boost/libraries/shared/math/Cargo.toml"),
        "[package]\nname = \"utility-kernel\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dev-dependencies]\nengine = { path = \"../../apps/engine\", package = \"boost-engine\", optional = true }\n",
    )
    .unwrap();
    let error = failure(&f, "E_CRATE_DEPENDENCY");
    assert_eq!(
        &*error.message,
        "same-bundle crate dependency cycle: engine -> math -> engine"
    );

    let f = sibling_fixture();
    manifest(&f, "boost", |v| {
        let third: toml::Value = toml::from_str(
            "[[crate]]\nid = \"codec\"\npath = \"libraries/tools/codec\"\ndependent = \"native-game\"\n",
        )
        .unwrap();
        v["crate"]
            .as_array_mut()
            .unwrap()
            .push(third["crate"].as_array().unwrap()[0].clone());
    });
    fs::write(
        f.mods.join("boost/libraries/shared/math/Cargo.toml"),
        "[package]\nname = \"utility-kernel\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[build-dependencies]\ncodec = { path = \"../../tools/codec\", package = \"codec-core\" }\n",
    )
    .unwrap();
    let codec = f.mods.join("boost/libraries/tools/codec");
    fs::create_dir_all(codec.join("src")).unwrap();
    fs::write(
        codec.join("Cargo.toml"),
        "[package]\nname = \"codec-core\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dev-dependencies]\nengine = { path = \"../../apps/engine\", package = \"boost-engine\" }\n",
    )
    .unwrap();
    fs::write(codec.join("src/lib.rs"), "pub fn encode() {}\n").unwrap();
    let error = failure(&f, "E_CRATE_DEPENDENCY");
    assert_eq!(
        &*error.message,
        "same-bundle crate dependency cycle: engine -> math -> codec -> engine"
    );
}

#[test]
fn stage_rejects_missing_extra_and_tampered_sibling_edges() {
    let f = sibling_fixture();
    let (mut plan, extras) = horizon_mixins_core::build_staged(&f.root, &f.mods).unwrap();
    plan.crates[0].dependencies.clear();
    let mut files = source::files(&f.root).unwrap();
    let error = horizon_mixins_core::native::stage(&plan, &mut files, extras).unwrap_err();
    assert_eq!(error.code, "E_CRATE_DEPENDENCY");
    assert!(error.message.contains("missing from the resolved plan"));

    let f = sibling_fixture();
    let (mut plan, extras) = horizon_mixins_core::build_staged(&f.root, &f.mods).unwrap();
    plan.crates[0]
        .dependencies
        .push(horizon_mixins_core::native::PlannedCrateDependency {
            table: "build-dependencies".into(),
            key: "extra_math".into(),
            target_crate_id: "math".into(),
        });
    let mut files = source::files(&f.root).unwrap();
    let error = horizon_mixins_core::native::stage(&plan, &mut files, extras).unwrap_err();
    assert_eq!(error.code, "E_CRATE_DEPENDENCY");
    assert!(
        error
            .message
            .contains("resolved path dependencies no longer match")
    );

    let f = sibling_fixture();
    let (mut plan, extras) = horizon_mixins_core::build_staged(&f.root, &f.mods).unwrap();
    plan.crates[0].dependencies[0].target_crate_id = "unknown".into();
    let mut files = source::files(&f.root).unwrap();
    let error = horizon_mixins_core::native::stage(&plan, &mut files, extras).unwrap_err();
    assert_eq!(error.code, "E_PLAN_SITE");

    let f = sibling_fixture();
    manifest(&f, "boost", |v| {
        let declaration: toml::Value = toml::from_str(
            "[[crate]]\nid = \"other\"\npath = \"libraries/shared/other\"\ndependent = \"native-game\"\n",
        )
        .unwrap();
        v["crate"]
            .as_array_mut()
            .unwrap()
            .push(declaration["crate"].as_array().unwrap()[0].clone());
    });
    let other = f.mods.join("boost/libraries/shared/other");
    fs::create_dir_all(other.join("src")).unwrap();
    fs::write(
        other.join("Cargo.toml"),
        "[package]\nname = \"other-kernel\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(other.join("src/lib.rs"), "pub fn other() {}\n").unwrap();
    let (mut plan, extras) = horizon_mixins_core::build_staged(&f.root, &f.mods).unwrap();
    plan.crates[0].dependencies[0].target_crate_id = "other".into();
    let mut files = source::files(&f.root).unwrap();
    let error = horizon_mixins_core::native::stage(&plan, &mut files, extras).unwrap_err();
    assert_eq!(error.code, "E_CRATE_DEPENDENCY");
    assert!(error.message.contains("names package 'utility-kernel'"));
    assert!(error.message.contains("package 'other-kernel'"));
}

#[test]
fn stage_rejects_changed_sibling_manifest_extras_before_rewriting() {
    let f = sibling_fixture();
    let (plan, mut extras) = horizon_mixins_core::build_staged(&f.root, &f.mods).unwrap();
    extras.insert(
        "native-crates/boost/engine/Cargo.toml".into(),
        b"[package]\nname = \"tampered\"\n".to_vec(),
    );
    let mut files = source::files(&f.root).unwrap();
    let error = horizon_mixins_core::native::stage(&plan, &mut files, extras).unwrap_err();
    assert_eq!(error.code, "E_STALE_SOURCE");
    assert!(!files.keys().any(|name| name.starts_with("native-crates/")));
}

#[test]
fn old_plan_schema_is_rejected_by_transform_validation() {
    let f = fixture();
    let mut plan = build_plan(&f.root, &f.mods).unwrap();
    plan.schema = 6;
    let error = plan.validate_structure().unwrap_err();
    assert_eq!(error.code, "E_PLAN_SCHEMA");
    assert!(error.message.contains("regenerate it with schema 7"));
}

#[cfg(unix)]
#[test]
fn intermediate_crate_roots_and_sibling_dependency_paths_reject_symlinks() {
    use std::os::unix::fs::symlink;

    let f = sibling_fixture();
    let libraries = f.mods.join("boost/libraries");
    fs::rename(libraries.join("shared"), libraries.join("shared-real")).unwrap();
    symlink("shared-real", libraries.join("shared")).unwrap();
    failure(&f, "E_INVALID_PATH");

    let engine_manifest =
        SIBLING_ENGINE_MANIFEST.replace("../../shared/math", "../../shared/math-link");
    let f = sibling_fixture_with(&engine_manifest);
    manifest(&f, "boost", |v| {
        v["crate"].as_array_mut().unwrap()[1]
            .as_table_mut()
            .unwrap()
            .insert("path".into(), "libraries/shared/math-real".into());
    });
    let shared = f.mods.join("boost/libraries/shared");
    fs::rename(shared.join("math"), shared.join("math-real")).unwrap();
    symlink("math-real", shared.join("math-link")).unwrap();
    let error = failure(&f, "E_CRATE_DEPENDENCY");
    assert!(error.message.contains("traverses a symlink"), "{error:?}");
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

use horizon_mixins_core::{build_plan, index::Index, source};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    mods: PathBuf,
    out: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("original");
        let mods = temp.path().join("mods");
        copy(&repo().join("fixtures/game"), &root);
        copy(&repo().join("fixtures/mods"), &mods);
        Self {
            out: temp.path().join("stage"),
            _temp: temp,
            root,
            mods,
        }
    }
    fn replace(&self, path: &str, from: &str, to: &str) {
        let path = match path.strip_prefix("mods/") {
            Some(p) => self.mods.join(p),
            None => self.root.join(path),
        };
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains(from),
            "{from} is absent in {}",
            path.display()
        );
        fs::write(path, text.replace(from, to)).unwrap();
    }
    fn append(&self, path: &str, text: &str) {
        let path = match path.strip_prefix("mods/") {
            Some(p) => self.mods.join(p),
            None => self.root.join(path),
        };
        let old = fs::read_to_string(&path).unwrap();
        fs::write(path, old + text).unwrap();
    }
    fn command(&self, action: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_horizon-mixins"));
        command
            .arg(action)
            .arg("--workspace")
            .arg(&self.root)
            .arg("--mods")
            .arg(&self.mods)
            .arg("--out")
            .arg(&self.out);
        if action == "check" {
            command.arg("--trust-native-mods");
        }
        command
    }
    fn run(&self, action: &str) -> serde_json::Value {
        success(self.command(action).output().unwrap())
    }
}
fn copy(from: &Path, to: &Path) {
    for (name, bytes) in source::files(from).unwrap() {
        let path = to.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}
fn success(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn mtimes(root: &Path) -> BTreeMap<String, std::time::SystemTime> {
    source::files(root)
        .unwrap()
        .keys()
        .map(|name| {
            (
                name.clone(),
                fs::metadata(root.join(name)).unwrap().modified().unwrap(),
            )
        })
        .collect()
}
#[test]
fn compiled_mvp_is_deterministic_preserves_sources_and_unchanged_mtimes() {
    let f = Fixture::new();
    let originals = source::files(&f.root).unwrap();
    let original_times = mtimes(&f.root);
    let first = f.run("check");
    assert_eq!(first["stage_changed"], true);
    assert!(first["cargo_stdout"].as_str().unwrap().contains("4 passed"));
    let stage = source::files(&f.out).unwrap();
    let times = mtimes(&f.out);
    assert_eq!(f.run("check")["stage_changed"], false);
    assert_eq!(source::files(&f.out).unwrap(), stage);
    assert_eq!(mtimes(&f.out), times);
    // A change in one hook publishes a new complete stage, retaining other files' mtimes.
    f.replace("mods/alpha/replace.rs", "input * 7", "input * 7 + 0");
    assert_eq!(f.run("check")["stage_changed"], true);
    let new_times = mtimes(&f.out);
    for (name, time) in &times {
        if name != "engine/src/replacement.rs" && name != ".horizon-mixins-stage.json" {
            assert_eq!(new_times[name], *time, "mtime changed for {name}");
        }
    }
    assert_eq!(source::files(&f.root).unwrap(), originals);
    assert_eq!(mtimes(&f.root), original_times);
    // Removed inputs disappear when the next complete workspace is published.
    fs::remove_file(f.root.join("README.md")).unwrap();
    f.run("apply");
    assert!(!f.out.join("README.md").exists());
}
#[test]
fn canonical_plan_and_source_snapshots_ignore_discovery_order() {
    let f = Fixture::new();
    let first = build_plan(&f.root, &f.mods).unwrap();
    fs::rename(f.mods.join("alpha"), f.mods.join("z-last-on-disk")).unwrap();
    fs::rename(f.mods.join("beta"), f.mods.join("a-first-on-disk")).unwrap();
    let second = build_plan(&f.root, &f.mods).unwrap();
    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&second).unwrap()
    );
    let serialized = serde_json::to_value(&first).unwrap();
    let head: Vec<_> = first
        .injections
        .iter()
        .filter(|p| p.target.ends_with("::advance"))
        .map(|p| p.mod_id.as_str())
        .collect();
    assert_eq!(head, ["alpha", "beta"]);
    let output = horizon_mixins_transform::transform(&f.root, &first).unwrap();
    let snapshots = repo().join("crates/horizon-mixins-cli/tests/snapshots");
    for (name, file) in [
        ("entity.rs", "engine/src/entity.rs"),
        ("calls.rs", "engine/src/calls.rs"),
        ("replacement.rs", "engine/src/replacement.rs"),
    ] {
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            fs::write(snapshots.join(name), &output[file]).unwrap();
        }
        assert_eq!(
            output[file],
            fs::read(snapshots.join(name)).unwrap(),
            "snapshot {name}"
        );
    }
    let mut portable_plan = serialized;
    portable_plan.as_object_mut().unwrap().remove("rustc");
    portable_plan.as_object_mut().unwrap().remove("inputs");
    let snapshot = serde_json::to_string_pretty(&portable_plan).unwrap() + "\n";
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        fs::write(snapshots.join("plan.json"), &snapshot).unwrap();
    }
    assert_eq!(
        snapshot,
        fs::read_to_string(snapshots.join("plan.json")).unwrap()
    );
}
#[test]
fn priority_ties_sort_by_mod_then_injection_id() {
    let f = Fixture::new();
    f.replace(
        "mods/beta/horizon-mixin.toml",
        "priority = 50",
        "priority = 100",
    );
    f.append("mods/alpha/horizon-mixin.toml", "\n[[injection]]\nid = \"aaa-head\"\ntarget = \"fixture_game::entity::advance\"\nkind = \"HEAD\"\nhelper = \"head.rs\"\nbindings = [{ name = \"entity\", ty = \"&mut Entity\", value = \"&mut *entity\" }]\n");
    let plan = build_plan(&f.root, &f.mods).unwrap();
    let order: Vec<_> = plan
        .injections
        .iter()
        .filter(|p| p.target.ends_with("::advance"))
        .map(|p| (p.mod_id.as_str(), p.injection_id.as_str()))
        .collect();
    assert_eq!(
        order,
        [("alpha", "aaa-head"), ("alpha", "head"), ("beta", "head")]
    );
}
#[test]
fn indexes_inline_external_path_inherent_and_trait_methods() {
    let f = Fixture::new();
    let index = Index::load(&f.root).unwrap();
    for target in [
        "fixture_game::entity::advance",
        "fixture_game::entity::Entity::tick",
        "fixture_game::entity::<Entity as Marker>::marked",
        "fixture_game::inline::identity",
        "fixture_game::inline::leaf::identity",
        "fixture_game::overwrite::value",
    ] {
        assert!(index.resolve(target).is_ok(), "missing {target}");
    }
    // Transform a nested inline/external module and actually compile its output.
    f.append("mods/alpha/horizon-mixin.toml", "\n[[injection]]\nid = \"inline\"\ntarget = \"fixture_game::inline::identity\"\nkind = \"HEAD\"\nhelper = \"nop.rs\"\n\n[[injection]]\nid = \"external\"\ntarget = \"fixture_game::inline::leaf::identity\"\nkind = \"HEAD\"\nhelper = \"nop.rs\"\n");
    fs::write(f.mods.join("alpha/nop.rs"), "fn hook() {}\n").unwrap();
    f.run("check");
}
#[test]
fn negative_planning_diagnostics_are_actionable() {
    let cases = [
        (
            "missing",
            "mods/alpha/horizon-mixin.toml",
            "fixture_game::entity::advance",
            "fixture_game::entity::missing",
            "E_TARGET_NOT_FOUND",
        ),
        (
            "selector missing",
            "mods/alpha/horizon-mixin.toml",
            "selector = \"consume\"",
            "selector = \"missing\"",
            "E_MATCH_COUNT",
        ),
        (
            "wrong count",
            "mods/alpha/horizon-mixin.toml",
            "require = 2",
            "require = 3",
            "E_MATCH_COUNT",
        ),
        (
            "wrong ordinal",
            "mods/alpha/horizon-mixin.toml",
            "ordinal = 1",
            "ordinal = 2",
            "E_ORDINAL",
        ),
        (
            "explicit count",
            "mods/alpha/horizon-mixin.toml",
            "require = 2",
            "",
            "E_SELECTOR",
        ),
        (
            "explicit ordinal",
            "mods/alpha/horizon-mixin.toml",
            "ordinal = 1",
            "",
            "E_SELECTOR",
        ),
        (
            "duplicate mod",
            "mods/beta/horizon-mixin.toml",
            "id = \"beta\"",
            "id = \"alpha\"",
            "E_DUPLICATE_ID",
        ),
        (
            "duplicate injection",
            "mods/alpha/horizon-mixin.toml",
            "id = \"tick\"",
            "id = \"head\"",
            "E_DUPLICATE_ID",
        ),
        (
            "traversal",
            "mods/alpha/horizon-mixin.toml",
            "helper = \"head.rs\"",
            "helper = \"../../escape.rs\"",
            "E_INVALID_PATH",
        ),
        (
            "invalid helper",
            "mods/alpha/head.rs",
            "fn head",
            "pub fn head",
            "E_INVALID_HELPER",
        ),
        (
            "helper syntax",
            "mods/alpha/head.rs",
            "fn head",
            "fn { head",
            "E_INVALID_HELPER",
        ),
        (
            "missing helper",
            "mods/alpha/horizon-mixin.toml",
            "helper = \"head.rs\"",
            "helper = \"absent.rs\"",
            "E_IO",
        ),
        (
            "binding name",
            "mods/alpha/horizon-mixin.toml",
            "name = \"entity\"",
            "name = \"absent\"",
            "E_INVALID_BINDING",
        ),
        (
            "binding local",
            "mods/alpha/horizon-mixin.toml",
            "value = \"&mut *entity\"",
            "value = \"&mut *missing\"",
            "E_INVALID_BINDING",
        ),
        (
            "binding call",
            "mods/alpha/horizon-mixin.toml",
            "value = \"&mut *entity\"",
            "value = \"Entity::new()\"",
            "E_INVALID_BINDING",
        ),
        (
            "cfg",
            "engine/src/entity.rs",
            "pub fn advance",
            "#[cfg(feature = \"unknown\")]\npub fn advance",
            "E_UNSUPPORTED_CFG",
        ),
        (
            "cfg module",
            "engine/src/lib.rs",
            "pub mod entity;",
            "#[cfg(feature = \"unknown\")]\npub mod entity;",
            "E_UNSUPPORTED_CFG",
        ),
        (
            "macro body with interior injection",
            "engine/src/calls.rs",
            "let first = consume(next(evaluations, log));",
            "unknown_macro!();",
            "E_UNSUPPORTED_MACRO",
        ),
        (
            "closure",
            "engine/src/calls.rs",
            "let first = consume(next(evaluations, log));",
            "let _closure = || 1; let first = consume(next(evaluations, log));",
            "E_UNSUPPORTED_CONTROL_FLOW",
        ),
        (
            "async",
            "engine/src/entity.rs",
            "pub fn advance",
            "pub async fn advance",
            "E_UNSUPPORTED_SIGNATURE",
        ),
        (
            "generic",
            "engine/src/entity.rs",
            "pub fn advance(",
            "pub fn advance<T>(",
            "E_UNSUPPORTED_SIGNATURE",
        ),
        (
            "rust syntax",
            "engine/src/entity.rs",
            "pub fn advance",
            "pub fn { advance",
            "E_RUST_SYNTAX",
        ),
        (
            "unknown manifest key",
            "mods/alpha/horizon-mixin.toml",
            "priority = 100",
            "priority = 100\nunknown = true",
            "E_MANIFEST",
        ),
        (
            "bad kind",
            "mods/alpha/horizon-mixin.toml",
            "kind = \"HEAD\"",
            "kind = \"REDIRECT\"",
            "E_MANIFEST",
        ),
    ];
    for (name, path, from, to, code) in cases {
        let f = Fixture::new();
        f.replace(path, from, to);
        let error = build_plan(&f.root, &f.mods).unwrap_err();
        assert_eq!(error.code, code, "{name}: {error:?}");
        assert!(!error.message.is_empty());
        assert!(error.file.is_some(), "{name}: missing file");
    }
}
#[test]
fn ambiguous_macro_generated_overwrite_and_fingerprint_failures() {
    let f = Fixture::new();
    f.append(
        "engine/src/entity.rs",
        "\npub fn advance(entity: &mut Entity) -> i32 { 0 }\n",
    );
    assert_eq!(
        build_plan(&f.root, &f.mods).unwrap_err().code,
        "E_TARGET_AMBIGUOUS"
    );
    let f = Fixture::new();
    f.append("engine/src/lib.rs", "\npub mod generated { macro_rules! generate { () => { pub fn hidden() {} }; } generate!(); }\n");
    f.replace(
        "mods/alpha/horizon-mixin.toml",
        "fixture_game::entity::advance",
        "fixture_game::generated::hidden",
    );
    assert_eq!(
        build_plan(&f.root, &f.mods).unwrap_err().code,
        "E_UNSUPPORTED_MACRO"
    );
    for target in [
        "fixture_game::entity::advance",
        "fixture_game::overwrite::value",
    ] {
        let f = Fixture::new();
        let (helper, name, ty, value) = if target.ends_with("advance") {
            ("head.rs", "entity", "&mut Entity", "&mut *entity")
        } else {
            ("replace.rs", "input", "i32", "input")
        };
        f.append("mods/alpha/horizon-mixin.toml", &format!("\n[[injection]]\nid = \"conflict\"\ntarget = \"{target}\"\nkind = \"OVERWRITE\"\nhelper = \"{helper}\"\nbindings = [{{ name = \"{name}\", ty = \"{ty}\", value = \"{value}\" }}]\n"));
        let error = build_plan(&f.root, &f.mods).unwrap_err();
        assert_eq!(error.code, "E_OVERWRITE_CONFLICT");
        assert!(error.message.contains("competing injections"));
        let output = f.command("apply").output().unwrap();
        assert!(!output.status.success());
        assert!(!f.out.exists());
    }
    let f = Fixture::new();
    f.replace(
        "mods/alpha/horizon-mixin.toml",
        "id = \"head\"",
        "id = \"head\"\nfingerprint = \"wrong\"",
    );
    assert_eq!(
        build_plan(&f.root, &f.mods).unwrap_err().code,
        "E_FINGERPRINT"
    );
    let f = Fixture::new();
    f.replace(
        "mods/alpha/horizon-mixin.toml",
        "[[injection]]",
        "[compatibility]\nhorizon_revision = \"wrong\"\n\n[[injection]]",
    );
    assert_eq!(build_plan(&f.root, &f.mods).unwrap_err().code, "E_MANIFEST");
}
#[test]
fn stale_source_is_rejected_before_rewrite() {
    let f = Fixture::new();
    let plan = build_plan(&f.root, &f.mods).unwrap();
    f.append("engine/src/entity.rs", "\n// changed\n");
    assert_eq!(
        horizon_mixins_transform::transform(&f.root, &plan)
            .unwrap_err()
            .code,
        "E_STALE_SOURCE"
    );
}
#[test]
fn compilation_reports_helper_borrow_and_return_errors() {
    for (file, text, message) in [
        (
            "head.rs",
            "fn head(entity: &mut Entity) { let moved = entity; entity.velocity += 1; let _ = moved; }",
            "moved value",
        ),
        (
            "replace.rs",
            "fn replace(input: i32) -> bool { input == 7 }",
            "mismatched types",
        ),
    ] {
        let f = Fixture::new();
        fs::write(f.mods.join("alpha").join(file), text).unwrap();
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
}
#[test]
fn cli_plan_is_json_and_validation_failure_keeps_prior_stage() {
    let mut f = Fixture::new();
    f.out = f._temp.path().join("plan.json");
    f.run("plan");
    let bytes = fs::read(&f.out).unwrap();
    let plan: horizon_mixins_core::Plan = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(plan.injections.len(), 8);
    let time = fs::metadata(&f.out).unwrap().modified().unwrap();
    f.run("plan");
    assert_eq!(fs::metadata(&f.out).unwrap().modified().unwrap(), time);
    f.out = f._temp.path().join("stage");
    f.run("apply");
    let old = source::files(&f.out).unwrap();
    f.replace(
        "mods/alpha/horizon-mixin.toml",
        "ordinal = 1",
        "ordinal = 999",
    );
    let output = f.command("apply").output().unwrap();
    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "E_ORDINAL");
    assert_eq!(error["mod_id"], "alpha");
    assert_eq!(error["injection_id"], "before-second");
    assert_eq!(error["selector"], "consume");
    assert_eq!(source::files(&f.out).unwrap(), old);
}
#[test]
fn output_protection_ownership_trust_and_missing_lockfile() {
    let mut f = Fixture::new();
    for out in [
        f.root.clone(),
        f.root.join("nested"),
        f._temp.path().to_path_buf(),
        f.mods.join("nested"),
    ] {
        f.out = out;
        let output = f.command("apply").output().unwrap();
        assert!(!output.status.success());
        let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["code"], "E_INVALID_PATH");
    }
    f.out = f._temp.path().join("unowned");
    fs::create_dir(&f.out).unwrap();
    fs::write(f.out.join("important"), "keep").unwrap();
    let output = f.command("apply").output().unwrap();
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "E_STAGE_OWNERSHIP");
    assert_eq!(fs::read_to_string(f.out.join("important")).unwrap(), "keep");
    let output = Command::new(env!("CARGO_BIN_EXE_horizon-mixins"))
        .args(["check", "--workspace"])
        .arg(&f.root)
        .arg("--mods")
        .arg(&f.mods)
        .arg("--out")
        .arg(&f.out)
        .output()
        .unwrap();
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "E_TRUST_NATIVE_MODS");
    fs::remove_file(f.root.join("Cargo.lock")).unwrap();
    assert_eq!(build_plan(&f.root, &f.mods).unwrap_err().code, "E_UNPINNED");
}
#[cfg(unix)]
#[test]
fn symlink_escapes_are_rejected() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    fs::remove_file(f.mods.join("alpha/head.rs")).unwrap();
    symlink(
        repo().join("fixtures/mods/alpha/head.rs"),
        f.mods.join("alpha/head.rs"),
    )
    .unwrap();
    assert_eq!(
        build_plan(&f.root, &f.mods).unwrap_err().code,
        "E_INVALID_PATH"
    );
    let f = Fixture::new();
    symlink(
        repo().join("fixtures/game/engine/src/entity.rs"),
        f.root.join("escape.rs"),
    )
    .unwrap();
    assert_eq!(
        build_plan(&f.root, &f.mods).unwrap_err().code,
        "E_INVALID_PATH"
    );
    let f = Fixture::new();
    symlink(&f.root, &f.out).unwrap();
    let output = f.command("apply").output().unwrap();
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "E_INVALID_PATH");
}
#[test]
fn a_canonicalized_output_path_stages() {
    // The Horizon launcher resolves links in its data folder with `canonicalize` (macOS's /tmp is
    // one). On Windows that gives a verbatim path, `\\?\C:\...`, whose first component is the drive.
    let mut f = Fixture::new();
    f.out = f
        .out
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap()
        .join("stage");
    assert_eq!(f.run("apply")["status"], "ok");
    assert!(f.out.join("engine/src/entity.rs").is_file());
}
#[test]
fn concurrent_checks_are_serialized_and_keep_a_complete_stage() {
    let f = Fixture::new();
    let first = f
        .command("check")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let second = f
        .command("check")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let a = success(first.wait_with_output().unwrap());
    let b = success(second.wait_with_output().unwrap());
    assert_ne!(a["stage_changed"], b["stage_changed"]);
    assert_eq!(a["target_dir"], b["target_dir"]);
    assert!(f.out.join("engine/src/entity.rs").is_file());
}

#[test]
fn cfg_attributes_shared_module_sources_and_shadowing_fail_closed() {
    for (path, from, to, code) in [
        (
            "engine/src/entity.rs",
            "pub fn advance",
            "#[instrument]\npub fn advance",
            "E_UNSUPPORTED_MACRO",
        ),
        (
            "engine/src/lib.rs",
            "pub mod entity;",
            "#[instrument]\npub mod entity;",
            "E_UNSUPPORTED_MACRO",
        ),
        (
            "engine/src/lib.rs",
            "pub mod entity;",
            "#![cfg(feature = \"unknown\")]\npub mod entity;",
            "E_UNSUPPORTED_CFG",
        ),
        (
            "engine/src/entity.rs",
            "pub fn advance",
            "#[cfg_attr(feature = \"unknown\", inline)]\npub fn advance",
            "E_UNSUPPORTED_CFG",
        ),
        (
            "engine/src/calls.rs",
            "let first = consume(next(evaluations, log));",
            "let mut log = Vec::new();\n    let first = consume(next(evaluations, log));",
            "E_INVALID_BINDING",
        ),
        (
            "engine/src/calls.rs",
            "let first = consume(next(evaluations, log));",
            "let _value = unknown()?; let first = consume(next(evaluations, log));",
            "E_UNSUPPORTED_CONTROL_FLOW",
        ),
        (
            "engine/src/calls.rs",
            "let first = consume(next(evaluations, log));",
            "let _value = &unknown(); let first = consume(next(evaluations, log));",
            "E_UNSUPPORTED_CONTROL_FLOW",
        ),
        (
            "engine/src/lib.rs",
            "pub mod entity;",
            "#[path = \"../escape.rs\"]\npub mod entity;",
            "E_INVALID_PATH",
        ),
    ] {
        let f = Fixture::new();
        f.replace(path, from, to);
        assert_eq!(build_plan(&f.root, &f.mods).unwrap_err().code, code, "{to}");
    }
    let f = Fixture::new();
    f.append(
        "engine/src/lib.rs",
        "\n#[path = \"entity.rs\"]\npub mod reused;\n",
    );
    assert_eq!(
        build_plan(&f.root, &f.mods).unwrap_err().code,
        "E_TARGET_AMBIGUOUS"
    );
    let f = Fixture::new();
    f.append(
        "engine/src/entity.rs",
        "\nimpl Another for Entity { fn marked(&self) -> i32 { 0 } }\n",
    );
    let index = Index::load(&f.root).unwrap();
    assert!(
        index
            .resolve("fixture_game::entity::<Entity as Marker>::marked")
            .is_ok()
    );
    assert!(
        index
            .resolve("fixture_game::entity::<Entity as Another>::marked")
            .is_ok()
    );
}

#[test]
fn exact_revision_pin_is_checked() {
    let f = Fixture::new();
    fs::write(
        f.root.join(".horizon-revision"),
        "1111111111111111111111111111111111111111\n",
    )
    .unwrap();
    f.replace("mods/alpha/horizon-mixin.toml", "[[injection]]\nid = \"head\"", "[compatibility]\nhorizon_revision = \"1111111111111111111111111111111111111111\"\n\n[[injection]]\nid = \"head\"");
    build_plan(&f.root, &f.mods).unwrap();
    f.replace(
        ".horizon-revision",
        "1111111111111111111111111111111111111111",
        "2222222222222222222222222222222222222222",
    );
    assert_eq!(build_plan(&f.root, &f.mods).unwrap_err().code, "E_REVISION");
}

#[test]
fn unsupported_temporary_scopes_and_malformed_ir_are_rejected() {
    let f = Fixture::new();
    f.replace(
        "engine/src/calls.rs",
        "let second = consume(next(evaluations, log));",
        "let second = consume(next(evaluations, log)) + 0;",
    );
    assert_eq!(
        build_plan(&f.root, &f.mods).unwrap_err().code,
        "E_UNSUPPORTED_SITE"
    );
    let f = Fixture::new();
    let mut plan = build_plan(&f.root, &f.mods).unwrap();
    let patch = plan
        .injections
        .iter_mut()
        .find(|p| p.call_site.is_some())
        .unwrap();
    patch.call_site = None;
    assert_eq!(
        horizon_mixins_transform::transform(&f.root, &plan)
            .unwrap_err()
            .code,
        "E_PLAN_SITE"
    );
}

#[test]
fn before_discarded_call_preserves_receiver_and_result_drop_order() {
    let f = Fixture::new();
    f.append(
        "engine/src/calls.rs",
        r#"
type Trace = std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>;
struct Guard { trace: Trace, name: &'static str }
impl Drop for Guard { fn drop(&mut self) { self.trace.borrow_mut().push(self.name); } }
impl Guard {
    fn produce(&self) -> Guard {
        self.trace.borrow_mut().push("call");
        Guard { trace: self.trace.clone(), name: "result-drop" }
    }
}
fn receiver(trace: &Trace) -> Guard {
    trace.borrow_mut().push("receiver");
    Guard { trace: trace.clone(), name: "receiver-drop" }
}
pub fn discarded(trace: &Trace) {
    receiver(trace).produce();
    trace.borrow_mut().push("after");
}
"#,
    );
    f.append("driver/src/lib.rs", r#"
#[test]
fn receiver_and_result_drop_order() {
    let trace = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    fixture_game::calls::discarded(&trace);
    assert_eq!(*trace.borrow(), ["hook", "receiver", "call", "result-drop", "receiver-drop", "after"]);
}
"#);
    fs::write(
        f.mods.join("alpha/drop.rs"),
        "fn before(trace: &Trace) { trace.borrow_mut().push(\"hook\"); }",
    )
    .unwrap();
    f.append("mods/alpha/horizon-mixin.toml", "\n[[injection]]\nid = \"drop\"\ntarget = \"fixture_game::calls::discarded\"\nkind = \"BEFORE_CALL\"\nselector = \"receiver(trace).produce\"\nordinal = 0\nrequire = 1\nhelper = \"drop.rs\"\nbindings = [{ name = \"trace\", ty = \"&Trace\", value = \"trace\" }]\n");
    let report = f.run("check");
    assert!(
        report["cargo_stdout"]
            .as_str()
            .unwrap()
            .contains("5 passed")
    );
}

#[path = "cases/advanced.rs"]
mod advanced;

#[path = "cases/scoped.rs"]
mod scoped;

#[path = "cases/boundary.rs"]
mod boundary;

#[path = "cases/effects.rs"]
mod effects;

#[path = "cases/standard_macros.rs"]
mod standard_macros;

#[path = "cases/destructured.rs"]
mod destructured;

#[path = "cases/native.rs"]
mod native;

#[path = "cases/returns.rs"]
mod returns;

#[path = "cases/variables.rs"]
mod variables;

#[test]
fn production_test_cfg_modules_are_indexed_and_applied() {
    let f = Fixture::new();
    f.replace(
        "engine/src/lib.rs",
        "pub mod entity;",
        "#[cfg(not(test))]\npub mod entity;",
    );
    fs::write(
        f.root.join("engine/src/main.rs"),
        "#[cfg(test)]\nmod entity;\nfn main() {}\n",
    )
    .unwrap();
    f.run("apply");
    let staged = fs::read_to_string(f.out.join("engine/src/entity.rs")).unwrap();
    assert!(staged.contains("alpha"));

    f.replace("engine/src/lib.rs", "#[cfg(not(test))]", "#[cfg(test)]");
    let error = build_plan(&f.root, &f.mods).unwrap_err();
    assert!(format!("{error:?}").contains("E_TARGET_NOT_FOUND"));
}

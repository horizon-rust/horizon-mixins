use super::*;

const HEADER: &str = "pub static HITS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);\n\
pub fn sink(n:i32)->i32 { n }\n\
pub fn apply(f: impl Fn(i32)->i32)->i32 { f(1) }\n";

/// A one-function engine, `run`, with the given body, and an `alpha` mod holding `injections`.
fn fixture(body: &str, injections: &str) -> Fixture {
    let f = Fixture::new();
    fs::remove_dir_all(&f.mods).unwrap();
    fs::create_dir_all(f.mods.join("alpha")).unwrap();
    fs::write(
        f.root.join("engine/src/lib.rs"),
        format!("{HEADER}pub fn run()->i32 {{ {body} }}"),
    )
    .unwrap();
    fs::write(f.root.join("driver/src/lib.rs"), "").unwrap();
    fs::write(
        f.mods.join("alpha/horizon-mixin.toml"),
        format!("[mod]\nid=\"alpha\"\nversion=\"1\"\n{injections}"),
    )
    .unwrap();
    fs::write(
        f.mods.join("alpha/before.rs"),
        "fn before() { crate::HITS.fetch_add(1, ::std::sync::atomic::Ordering::SeqCst); }",
    )
    .unwrap();
    fs::write(
        f.mods.join("alpha/bump.rs"),
        "fn bump(value: usize) -> usize { value + 10 }",
    )
    .unwrap();
    f
}
fn before(id: &str, selector: &str, require: usize, ordinal: usize) -> String {
    format!(
        "\n[[injection]]\nid=\"{id}\"\ntarget=\"fixture_game::run\"\nkind=\"BEFORE_CALL\"\nhelper=\"before.rs\"\nselector=\"{selector}\"\nrequire={require}\nordinal={ordinal}\n"
    )
}
fn bump(selector: &str) -> String {
    format!(
        "\n[[injection]]\nid=\"bump\"\ntarget=\"fixture_game::run\"\nkind=\"MODIFY_EXPRESSION_VALUE\"\nhelper=\"bump.rs\"\nbindings=[{{name=\"value\",ty=\"usize\",value=\"$value\"}}]\nselector=\"{selector}\"\nordinal=0\nrequire=1\n"
    )
}
fn reject(f: &Fixture, code: &str) {
    let e = build_plan(&f.root, &f.mods).unwrap_err();
    assert_eq!(e.code, code, "{e}");
    assert!(e.file.is_some());
    assert!(!f.out.exists());
}

const BODY: &str = "
    use std::vec::Vec;
    let values = [1, 2, 3];
    let doubled: Vec<i32> = values.iter().map(|x| x + 1).collect();
    let applied = apply(|x| x + 1);
    let n = sink(doubled.len() as i32);
    let size = values.len();
    n + doubled[0] + size as i32 + applied
";

#[test]
fn site_hooks_run_in_functions_with_local_use_and_closures_elsewhere() {
    let f = fixture(
        BODY,
        &format!(
            "{}{}{}",
            before("before-sink", "sink", 1, 0),
            // The selected call itself takes a closure: BEFORE_CALL only runs a helper first.
            before("before-apply", "apply", 1, 0),
            bump("values.len()"),
        ),
    );
    // doubled = [2,3,4]; applied = 2; n = 3; size = 3 + 10; 3 + 2 + 13 + 2.
    fs::write(
        f.root.join("driver/src/lib.rs"),
        "#[test] fn local_scope() {\n\
         assert_eq!(fixture_game::run(), 20);\n\
         assert_eq!(fixture_game::HITS.load(std::sync::atomic::Ordering::SeqCst), 2);\n}",
    )
    .unwrap();
    f.run("check");
}
#[test]
fn return_hooks_accept_local_use() {
    let f = fixture(
        "use std::vec::Vec; let v: Vec<i32> = Vec::new(); v.len() as i32",
        "\n[[injection]]\nid=\"return\"\ntarget=\"fixture_game::run\"\nkind=\"RETURN\"\nhelper=\"plus.rs\"\nbindings=[{name=\"value\",ty=\"i32\",value=\"$value\"}]\n",
    );
    fs::write(
        f.mods.join("alpha/plus.rs"),
        "fn plus(value:i32)->i32 { value+4 }",
    )
    .unwrap();
    fs::write(
        f.root.join("driver/src/lib.rs"),
        "#[test] fn t() { assert_eq!(fixture_game::run(), 4); }",
    )
    .unwrap();
    f.run("check");
}
#[test]
fn a_nested_receiver_call_is_still_refused_but_for_its_own_reason() {
    // Issue #19: `values.iter` inside a chain that holds a closure. The closure is no longer
    // the obstacle; BEFORE_CALL still needs a complete statement, let initializer or tail.
    let f = fixture(
        "use std::vec::Vec; let values: Vec<i32> = Vec::new(); let n = values.iter().map(|x| x + 1).count(); n as i32",
        &before("before", "values.iter", 1, 0),
    );
    reject(&f, "E_UNSUPPORTED_SITE");
    // The same call as a complete let initializer plans with the local `use`.
    let f = fixture(
        "use std::vec::Vec; let values: Vec<i32> = Vec::new(); let it = values.iter(); let n = it.map(|x| x + 1).count(); n as i32",
        &before("before", "values.iter", 1, 0),
    );
    build_plan(&f.root, &f.mods).unwrap();
}
#[test]
fn sites_in_closures_and_other_unsupported_scopes_stay_refused() {
    let body = "let f = || sink(1); let a = sink(2); f() + a";
    let f = fixture(body, &before("before", "sink", 2, 0));
    reject(&f, "E_UNSUPPORTED_SITE");
    // The second match is outside the closure and is selectable.
    let f = fixture(body, &before("before", "sink", 2, 1));
    build_plan(&f.root, &f.mods).unwrap();
    // A selected expression that contains a closure cannot be moved into a helper argument.
    let f = fixture(
        "let values = [1]; let n = values.iter().map(|x| x + 1).count(); n as i32",
        &bump("values.iter().map(|x| x + 1).count()"),
    );
    reject(&f, "E_UNSUPPORTED_SITE");
    // Async closures, `?` and nested items are as unsupported as before.
    for body in [
        "let f = async || 1; sink(1)",
        "let f = || Some(1)?; sink(1)",
        "fn inner() {} sink(1)",
    ] {
        let f = fixture(body, &before("before", "sink", 1, 0));
        reject(&f, "E_UNSUPPORTED_CONTROL_FLOW");
    }
}
#[test]
fn local_use_that_could_change_generated_code_is_refused() {
    for (uses, code) in [
        ("use std::vec::*;", "E_UNSUPPORTED_CONTROL_FLOW"),
        (
            "pub(crate) use std::vec::Vec;",
            "E_UNSUPPORTED_CONTROL_FLOW",
        ),
        ("#[cfg(unix)] use std::vec::Vec;", "E_UNSUPPORTED_CFG"),
        // The helper mentions `Vec` and `atomic`: rebinding either would change its meaning.
        ("use std::vec::Vec;", "E_UNSUPPORTED_SCOPE"),
        ("use std::sync::atomic;", "E_UNSUPPORTED_SCOPE"),
        (
            "use std::sync::atomic::{self as atomic};",
            "E_UNSUPPORTED_SCOPE",
        ),
    ] {
        let f = fixture(&format!("{uses} sink(1)"), &before("before", "sink", 1, 0));
        if code == "E_UNSUPPORTED_SCOPE" {
            fs::write(
                f.mods.join("alpha/before.rs"),
                "fn before() { let _: Vec<i32> = Vec::new(); let _ = atomic::Ordering::SeqCst; }",
            )
            .unwrap();
        }
        reject(&f, code);
    }
    // A `use` that shadows a standard macro is seen even when the macro call comes first.
    let f = fixture(
        "let _ = vec![1]; use other::vec; sink(1)",
        &before("before", "sink", 1, 0),
    );
    reject(&f, "E_UNSUPPORTED_MACRO");
    // OVERWRITE rewrites the whole body and keeps its old restriction.
    let f = fixture(
        "use std::vec::Vec; sink(1)",
        "\n[[injection]]\nid=\"overwrite\"\ntarget=\"fixture_game::run\"\nkind=\"OVERWRITE\"\nhelper=\"over.rs\"\n",
    );
    fs::write(f.mods.join("alpha/over.rs"), "fn over() -> i32 { 1 }").unwrap();
    reject(&f, "E_UNSUPPORTED_CONTROL_FLOW");
}

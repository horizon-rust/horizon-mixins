use super::*;

const ENGINE: &str = "pub static SEEN: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);\n\
pub struct Point { pub x: i32, pub y: i32 }\n\
pub fn sink(n: i32) -> i32 { n }\n\
pub fn run(base: i32, (a, b): (i32, i32), Point { x, mut y }: Point) -> i32 {\n\
    y += 1;\n\
    base + a * b + x + y\n\
}\n";

fn fixture(injections: &str) -> Fixture {
    let f = Fixture::new();
    fs::remove_dir_all(&f.mods).unwrap();
    fs::create_dir_all(f.mods.join("alpha")).unwrap();
    fs::write(f.root.join("engine/src/lib.rs"), ENGINE).unwrap();
    fs::write(f.root.join("driver/src/lib.rs"), "").unwrap();
    fs::write(
        f.mods.join("alpha/horizon-mixin.toml"),
        format!("[mod]\nid=\"alpha\"\nversion=\"1\"\n{injections}"),
    )
    .unwrap();
    for (name, source) in [
        (
            "seen.rs",
            "fn seen(a: i32, b: i32, x: i32) { crate::SEEN.fetch_add(a * 100 + b * 10 + x, ::std::sync::atomic::Ordering::SeqCst); }",
        ),
        ("grow.rs", "fn grow(y: &mut i32) { *y += 100; }"),
        (
            "plus.rs",
            "fn plus(value: i32, a: i32) -> i32 { value + a }",
        ),
    ] {
        fs::write(f.mods.join("alpha").join(name), source).unwrap();
    }
    f
}
fn injection(id: &str, kind: &str, helper: &str, bindings: &str) -> String {
    format!(
        "\n[[injection]]\nid=\"{id}\"\ntarget=\"fixture_game::run\"\nkind=\"{kind}\"\nhelper=\"{helper}\"\nbindings=[{bindings}]\n"
    )
}
fn reject(f: &Fixture, code: &str) {
    let e = build_plan(&f.root, &f.mods).unwrap_err();
    assert_eq!(e.code, code, "{e}");
    assert!(e.file.is_some());
    assert!(!f.out.exists());
}

#[test]
fn hooks_bind_the_names_of_destructured_parameters() {
    let f = fixture(&format!(
        "{}{}{}",
        injection(
            "seen",
            "HEAD",
            "seen.rs",
            r#"{name="a",ty="i32",value="a"},{name="b",ty="i32",value="b"},{name="x",ty="i32",value="x"}"#,
        ),
        // `y` is declared `mut` inside the pattern, so a hook may borrow it mutably.
        injection(
            "grow",
            "HEAD",
            "grow.rs",
            r#"{name="y",ty="&mut i32",value="&mut y"}"#,
        ),
        injection(
            "plus",
            "RETURN",
            "plus.rs",
            r#"{name="value",ty="i32",value="$value"},{name="a",ty="i32",value="a"}"#,
        ),
    ));
    // base 1 + 2 * 3 + x 4 + (y 5 + 100 grown + 1) = 117, plus the RETURN hook's `a` = 119.
    fs::write(
        f.root.join("driver/src/lib.rs"),
        "use fixture_game::{Point, SEEN, run};\n\
         #[test] fn destructured() {\n\
         assert_eq!(run(1, (2, 3), Point { x: 4, y: 5 }), 119);\n\
         assert_eq!(SEEN.load(std::sync::atomic::Ordering::SeqCst), 234);\n}",
    )
    .unwrap();
    f.run("check");
}
#[test]
fn destructured_parameters_keep_their_other_restrictions() {
    // The unnamed whole of a destructured parameter cannot be bound.
    let f = fixture(&injection(
        "seen",
        "HEAD",
        "grow.rs",
        r#"{name="y",ty="&mut i32",value="&mut pair"}"#,
    ));
    reject(&f, "E_INVALID_BINDING");
    // A name the pattern binds is shadowed where a RETURN exit reads it.
    let f = fixture(&injection(
        "plus",
        "RETURN",
        "plus.rs",
        r#"{name="value",ty="i32",value="$value"},{name="a",ty="i32",value="a"}"#,
    ));
    f.replace(
        "engine/src/lib.rs",
        "base + a * b + x + y",
        "let a = 100; base + a * b + x + y",
    );
    reject(&f, "E_INVALID_BINDING");
    // A helper may not take the name of a pattern-bound parameter.
    let f = fixture(&injection(
        "seen",
        "HEAD",
        "seen.rs",
        r#"{name="a",ty="i32",value="a"},{name="b",ty="i32",value="b"},{name="x",ty="i32",value="x"}"#,
    ));
    fs::write(
        f.mods.join("alpha/seen.rs"),
        "fn a(a: i32, b: i32, x: i32) {}",
    )
    .unwrap();
    reject(&f, "E_INVALID_HELPER");
    // Pattern macros are still refused.
    let f = fixture(&injection(
        "seen",
        "HEAD",
        "seen.rs",
        r#"{name="a",ty="i32",value="a"},{name="b",ty="i32",value="b"},{name="x",ty="i32",value="x"}"#,
    ));
    f.replace(
        "engine/src/lib.rs",
        "(a, b): (i32, i32)",
        "pair!(a, b): (i32, i32)",
    );
    reject(&f, "E_UNSUPPORTED_MACRO");
}

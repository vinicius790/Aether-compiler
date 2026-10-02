//! Audit regressions for file modules, `pub` visibility (E0281), `fmt`
//! round-trips, odd inputs, diagnostics rendering and the CLI front end.
//! Programs that run are executed at `-O0` and `-O2` and must agree.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_aether"));
    c.current_dir(env!("CARGO_MANIFEST_DIR"));
    c.env_remove("AETHER_INCLUDE").env_remove("NO_COLOR");
    c
}

fn run(args: &[&str]) -> Output {
    bin().args(args).output().expect("spawn aether")
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn path_has(tool: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(tool).is_file()))
        .unwrap_or(false)
}

/// Fresh scratch directory under the system temp dir (std only).
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "aether-audit-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, src: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, src).unwrap();
    p.display().to_string()
}

/// A `lib.ae` with one of everything, public and private.
const LIB: &str = "\
fn hidden(x: i32) -> i32 { return x + 1; }
pub fn shown(x: i32) -> i32 { return hidden(x) * 2; }
struct Priv { v: i32, }
pub struct Pub { v: i32, w: f64, }
pub fn mk() -> Pub { return Pub { v: 1, w: 2.5 }; }
pub fn mkp() -> Priv { return Priv { v: 9 }; }
pub extern fn host(x: i32) -> i32;
extern fn host2(x: i32) -> i32;
enum Color { Red, Green }
pub fn red() -> Color { return Color::Red; }
pub fn tup() -> (Priv, i32) { return (Priv { v: 1 }, 2); }
";

/// `check` a main file that imports `lib.ae` (= [`LIB`]) and return stderr
/// after asserting that it fails with exactly one E0281 at `line:col`.
fn expect_private(main_src: &str, name: &str, line: u32, col: u32) -> String {
    let d = scratch("priv");
    write(&d, "lib.ae", LIB);
    let main = write(&d, "main.ae", &format!("use \"lib\";\n{main_src}\n"));
    let o = run(&["check", &main]);
    assert_eq!(o.status.code(), Some(1), "{}", err(&o));
    let e = err(&o);
    assert_eq!(e.matches("E0281").count(), 1, "{e}");
    assert!(e.contains(&format!("`{name}` is private to `")), "{e}");
    assert!(e.contains("lib.ae`"), "{e}");
    assert!(e.contains(&format!("main.ae:{line}:{col}")), "{e}");
    assert!(e.contains("help: mark it `pub` in "), "{e}");
    let _ = std::fs::remove_dir_all(&d);
    e
}

fn run_both(file: &str, extra: &[&str]) -> String {
    let mut a = vec!["run", file, "-O0"];
    a.extend_from_slice(extra);
    let o0 = run(&a);
    a[2] = "-O2";
    let o2 = run(&a);
    assert_eq!(o0.status.code(), o2.status.code(), "{}\n{}", err(&o0), err(&o2));
    assert_eq!(out(&o0), out(&o2));
    assert!(o0.status.success(), "{}", err(&o0));
    out(&o0)
}

// ---------------------------------------------------------------------------
// pub visibility
// ---------------------------------------------------------------------------

#[test]
fn private_function_from_another_file_is_e0281() {
    let e = expect_private("fn main() -> i32 { return hidden(1); }", "hidden", 2, 27);
    assert!(e.contains("function `hidden` is defined at"), "{e}");
}

#[test]
fn private_struct_literal_type_array_and_param_are_e0281() {
    expect_private(
        "fn main() -> i32 { let p = Priv { v: 1 }; return p.v; }",
        "Priv",
        2,
        28,
    );
    expect_private("fn take(p: Priv) -> i32 { return p.v; }\nfn main() -> i32 { return 0; }", "Priv", 2, 12);
    expect_private(
        "fn main() -> i32 { let q: [Priv; 2] = [mkp(), mkp()]; return 0; }",
        "Priv",
        2,
        28,
    );
    expect_private(
        "fn main() -> i32 { let t: (Priv, i32) = tup(); return t.1; }",
        "Priv",
        2,
        28,
    );
}

#[test]
fn private_extern_is_e0281_pub_extern_is_not() {
    expect_private("fn main() -> i32 { return host2(1); }", "host2", 2, 27);
    let d = scratch("ext");
    write(&d, "lib.ae", LIB);
    let main = write(&d, "main.ae", "use \"lib\";\nfn main() -> i32 { return host(1); }\n");
    let o = run(&["check", &main]);
    assert!(o.status.success(), "{}", err(&o));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn pub_items_are_usable_and_values_of_private_types_flow_through() {
    let d = scratch("pubok");
    write(&d, "lib.ae", LIB);
    let main = write(
        &d,
        "main.ae",
        "use \"lib\";
fn main() -> i32 {
    print_i32(shown(1));
    let p = mk();
    print_i32(p.v);
    print_f64(p.w);
    let q = Pub { v: 3, w: 1.0 };
    print_i32(q.v);
    // a value of a private type can be held and read without naming the type
    let h = mkp();
    print_i32(h.v);
    let (a, b) = tup();
    print_i32(a.v + b);
    // enums are not subject to `pub`
    let c = red();
    match c { Color::Red => { print_i32(100); } Color::Green => { print_i32(200); } }
    return 0;
}
",
    );
    assert_eq!(run_both(&main, &[]), "4\n1\n2.5\n3\n9\n3\n100\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn private_items_of_the_same_file_are_always_accessible() {
    let d = scratch("same");
    let main = write(
        &d,
        "main.ae",
        "struct S { x: i32, }\nfn helper(s: S) -> i32 { return s.x; }\n\
         fn main() -> i32 { print_i32(helper(S { x: 4 })); return 0; }\n",
    );
    assert_eq!(run_both(&main, &[]), "4\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn imports_are_flat_and_transitive_for_pub_items_only() {
    let d = scratch("trans");
    write(&d, "c.ae", "pub fn cpub() -> i32 { return 3; }\nfn cpriv() -> i32 { return 30; }\n");
    write(
        &d,
        "b.ae",
        "use \"c\";\npub fn bpub() -> i32 { return cpub() + cpriv_user(); }\n\
         fn cpriv_user() -> i32 { return 1; }\n",
    );
    let main = write(
        &d,
        "a.ae",
        "use \"b\";\nfn main() -> i32 { print_i32(bpub()); print_i32(cpub()); return 0; }\n",
    );
    // `a` sees the pub items of `b` and of `c` (flat namespace)
    assert_eq!(run_both(&main, &[]), "4\n3\n");
    // but not the private ones of either
    let bad = write(
        &d,
        "bad.ae",
        "use \"b\";\nfn main() -> i32 { return cpriv() + cpriv_user(); }\n",
    );
    let o = run(&["check", &bad]);
    assert_eq!(o.status.code(), Some(1));
    let e = err(&o);
    assert!(e.contains("`cpriv` is private to"), "{e}");
    assert!(e.contains("`cpriv_user` is private to"), "{e}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn visibility_applies_to_include_in_both_directions() {
    let d = scratch("incl");
    let lib = write(
        &d,
        "inc.ae",
        "pub fn api() -> i32 { return from_main(); }\nfn internal() -> i32 { return 1; }\n",
    );
    let main = write(
        &d,
        "main.ae",
        "fn from_main() -> i32 { return 2; }\nfn main() -> i32 { return api(); }\n",
    );
    // the included file may not call the main file's private function...
    let o = run(&["check", &main, "--include", &lib]);
    assert_eq!(o.status.code(), Some(1));
    let e = err(&o);
    assert!(e.contains("`from_main` is private to"), "{e}");
    assert!(e.contains("inc.ae:1:"), "{e}");
    // ...and the main file may not call the included file's private one
    let main2 = write(&d, "main2.ae", "fn main() -> i32 { return internal(); }\n");
    let o = run(&["check", &main2, "--include", &lib]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("`internal` is private to"), "{}", err(&o));
    // pub on both sides works
    let lib_ok = write(&d, "inc_ok.ae", "pub fn api() -> i32 { return from_main(); }\n");
    let main_ok = write(
        &d,
        "main_ok.ae",
        "pub fn from_main() -> i32 { return 2; }\nfn main() -> i32 { print_i32(api()); return 0; }\n",
    );
    assert_eq!(run_both(&main_ok, &["--include", &lib_ok]), "2\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_private_use_site_is_reported_once() {
    let d = scratch("once");
    write(&d, "lib.ae", LIB);
    // the same private struct named in a parameter and in the return type:
    // one error per distinct use site (signatures are checked before bodies)
    let main = write(
        &d,
        "main.ae",
        "use \"lib\";\nfn f(a: Priv) -> Priv { return a; }\nfn main() -> i32 { return 0; }\n",
    );
    let o = run(&["check", &main]);
    let e = err(&o);
    assert_eq!(e.matches("E0281").count(), 2, "{e}");
    // and in a body: the annotation and the literal are separate sites
    let main = write(
        &d,
        "main2.ae",
        "use \"lib\";\nfn main() -> i32 { let b: Priv = Priv { v: 1 }; return b.v; }\n",
    );
    let e = err(&run(&["check", &main]));
    assert_eq!(e.matches("E0281").count(), 2, "{e}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn private_helpers_with_the_same_name_in_two_files_still_clash() {
    // documented limitation: the namespace is flat even for private items
    let d = scratch("clash");
    write(&d, "lib.ae", "fn helper() -> i32 { return 1; }\npub fn one() -> i32 { return helper(); }\n");
    let main = write(
        &d,
        "main.ae",
        "use \"lib\";\nfn helper() -> i32 { return 2; }\nfn main() -> i32 { return one() + helper(); }\n",
    );
    let o = run(&["check", &main]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("duplicate function `helper`"), "{}", err(&o));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn stdlib_files_export_pub_api_and_still_run_standalone() {
    for f in ["math", "cmp", "loops", "bits"] {
        let path = format!("stdlib/{f}.ae");
        let o0 = run(&["run", &path, "-O0"]);
        let o2 = run(&["run", &path, "-O2"]);
        assert!(o0.status.success(), "{f}: {}", err(&o0));
        assert_eq!(out(&o0), out(&o2), "{f}");
    }
    let d = scratch("stdlib");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("stdlib");
    let main = write(
        &d,
        "main.ae",
        &format!(
            "use {:?};\nuse {:?};\nuse {:?};\nfn main() -> i32 {{\n\
             let v = vec2_scale(vec2_new(1.0, 2.0), 2.0);\n print_f64(v.y);\n\
             print_i32(rng_range(rng_next(1), 0, 10));\n print_i32(gcd(12, 18));\n return 0;\n}}\n",
            root.join("vec2.ae").display().to_string(),
            root.join("rng.ae").display().to_string(),
            root.join("prelude.ae").display().to_string(),
        ),
    );
    let text = run_both(&main, &[]);
    assert!(text.starts_with("4\n"), "{text}");
    assert!(text.ends_with("6\n"), "{text}");
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// fmt
// ---------------------------------------------------------------------------

#[test]
fn fmt_round_trips_floats_let_types_and_is_idempotent() {
    let d = scratch("fmt");
    let src = "\
fn main() -> i32 {
    let a = 3.0;
    let b: f64 = 1e10;
    let c = 2.5e-3;
    let d = -4.0;
    let e = 100.0 / 3.0;
    let n = -2147483648;
    let big: i64 = 5000000000;
    let t: (i32, f64) = (1, 2.0);
    let arr: [f64; 2] = [1.0, 2.0];
    let z = 1.0e300 * 1.0e300;
    print_f64(a); print_f64(b); print_f64(c); print_f64(d); print_f64(e);
    print_i32(n); print_i64(big); print_f64(t.1); print_f64(arr[1]); print_f64(z);
    return 0;
}
";
    let f = write(&d, "f.ae", src);
    let formatted = out(&run(&["fmt", &f]));
    assert!(formatted.contains("let a = 3.0;"), "{formatted}");
    assert!(formatted.contains("let big: i64 = 5000000000;"), "{formatted}");
    assert!(formatted.contains("let t: (i32, f64) = "), "{formatted}");
    assert!(!formatted.contains("((-"), "{formatted}");
    let g = write(&d, "g.ae", &formatted);
    assert_eq!(run_both(&f, &[]), run_both(&g, &[]));
    // idempotent
    assert_eq!(out(&run(&["fmt", &g])), formatted);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn fmt_prints_only_the_main_file_and_keeps_use_lines() {
    let o = run(&["fmt", "examples/modules.ae"]);
    assert!(o.status.success(), "{}", err(&o));
    let text = out(&o);
    assert!(text.contains("\nuse \"../stdlib/vec2.ae\";\nuse \"../stdlib/rng.ae\";\n"), "{text}");
    assert!(!text.contains("struct Vec2"), "{text}");
    assert!(!text.contains("fn rng_next"), "{text}");
    // the formatted program, next to the original, behaves the same
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let g = d.join("zz_audit_fmt_modules.ae");
    std::fs::write(&g, &text).unwrap();
    let a = out(&run(&["run", "examples/modules.ae"]));
    let b = out(&run(&["run", g.to_str().unwrap()]));
    let _ = std::fs::remove_file(&g);
    assert_eq!(a, b);
}

#[test]
fn fmt_rejects_syntax_errors_but_not_type_errors() {
    let d = scratch("fmterr");
    let bad = write(&d, "bad.ae", "fn main() -> i32 { let x = ; }\n");
    let o = run(&["fmt", &bad]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("expected expression"), "{}", err(&o));
    assert_eq!(out(&o), "");
    let typed = write(&d, "typed.ae", "fn main() -> i32 { let x: i32 = \"s\"; return 0; }\n");
    let o = run(&["fmt", &typed]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("let x: i32 = \"s\";"), "{}", out(&o));
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// odd inputs
// ---------------------------------------------------------------------------

#[test]
fn empty_and_comment_only_files_report_the_missing_entry_point() {
    let d = scratch("empty");
    for (name, src) in [("e.ae", ""), ("c.ae", "// nothing here\n/* nor here */\n")] {
        let f = write(&d, name, src);
        let o = run(&["run", &f]);
        assert_eq!(o.status.code(), Some(1));
        assert!(err(&o).contains("E0210"), "{}", err(&o));
        assert!(!err(&o).contains("panicked"), "{}", err(&o));
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn utf8_bom_is_ignored() {
    let d = scratch("bom");
    let f = d.join("bom.ae");
    std::fs::write(&f, b"\xef\xbb\xbffn main() -> i32 { print_i32(1); return 0; }\n").unwrap();
    assert_eq!(run_both(f.to_str().unwrap(), &[]), "1\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn crlf_sources_run_and_point_at_the_right_line_and_column() {
    let d = scratch("crlf");
    let f = write(
        &d,
        "crlf.ae",
        "fn main() -> i32 {\r\n  print_i32(1);\r\n  let x: i32 = \"a\";\r\n  return 0;\r\n}\r\n",
    );
    let o = run(&["check", &f]);
    let e = err(&o);
    assert!(e.contains("crlf.ae:3:3"), "{e}");
    assert!(!e.contains('\r'), "{e}");
    assert!(e.contains("   3 |   let x: i32 = \"a\";\n"), "{e}");
    let ok = write(&d, "ok.ae", "fn main() -> i32 {\r\n  print(\"a\\r\");\r\n  return 0;\r\n}\r\n");
    assert!(run_both(&ok, &[]).starts_with('a'));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn carets_line_up_under_tabs_and_multibyte_characters() {
    let d = scratch("caret");
    let tab = write(&d, "tab.ae", "fn main() -> i32 {\n\tlet x: i32 = \"a\";\n\treturn 0;\n}\n");
    let e = err(&run(&["check", &tab]));
    // the tab is copied into the caret line, so any tab width lines up
    assert!(e.contains("2 | \tlet x: i32 = \"a\";\n"), "{e}");
    assert!(e.contains("| \t^^^^^^^^^^^^^^^^^\n"), "{e}");
    // columns count characters, and the caret run counts characters too
    let mb = write(
        &d,
        "mb.ae",
        "fn main() -> i32 {\n  let s = \"héllo wörld\"; let x: i32 = s;\n  return 0;\n}\n",
    );
    let e = err(&run(&["check", &mb]));
    assert!(e.contains("mb.ae:2:26"), "{e}");
    let lines: Vec<&str> = e.lines().collect();
    let src_idx = lines.iter().position(|l| l.contains("héllo")).unwrap();
    let caret_line = lines[src_idx + 1];
    let src_line = lines[src_idx];
    // same number of characters before the first caret as before `let x`
    let before_src = src_line.chars().take_while(|c| *c != 'x').count() - "let ".len();
    let before_caret = caret_line.chars().take_while(|c| *c != '^').count();
    assert_eq!(before_src, before_caret, "{e}");
    assert_eq!(caret_line.matches('^').count(), "let x: i32 = s;".chars().count(), "{e}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn unicode_identifiers_are_a_diagnostic_not_a_panic() {
    let d = scratch("uni");
    for src in [
        "fn main() -> i32 {\n  let π = 3;\n  return 0;\n}\n",
        "fn 日本() -> i32 { return 0; }\nfn main() -> i32 { return 0; }\n",
        "fn main() -> i32 { let s = \"日本語\"; let é = 1; return 0; }\n",
    ] {
        let f = write(&d, "u.ae", src);
        let o = run(&["run", &f]);
        assert_eq!(o.status.code(), Some(1), "{src}");
        let e = err(&o);
        assert!(e.contains("unexpected character"), "{e}");
        assert!(!e.contains("panicked"), "{e}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn use_inside_a_function_is_a_parse_error() {
    let d = scratch("usefn");
    let f = write(&d, "u.ae", "fn f() -> i32 { use \"x\"; return 0; }\nfn main() -> i32 { return 0; }\n");
    let o = run(&["check", &f]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("E0100"), "{}", err(&o));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn unreadable_inputs_give_one_clear_line() {
    let d = scratch("unread");
    let bad = d.join("bad.ae");
    std::fs::write(&bad, b"fn main() -> i32 { return 0; }\n\xff\xfe\n").unwrap();
    let o = run(&["run", bad.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    let e = err(&o);
    assert!(e.contains("not valid UTF-8"), "{e}");
    assert!(e.contains("offset 31"), "{e}");
    assert_eq!(e.lines().count(), 1, "{e}");

    let o = run(&["run", d.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("Is a directory"), "{}", err(&o));

    let o = run(&["run", d.join("missing.ae").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("cannot read"), "{}", err(&o));

    // `use` of a directory, of a missing file and of a non-UTF-8 file
    std::fs::create_dir_all(d.join("sub.ae")).unwrap();
    for (n, imp) in [("d", "sub"), ("m", "nope"), ("b", "bad")] {
        let f = write(&d, &format!("{n}.ae"), &format!("use \"{imp}\";\nfn main() -> i32 {{ return 0; }}\n"));
        let o = run(&["check", &f]);
        assert_eq!(o.status.code(), Some(1), "{imp}");
        let e = err(&o);
        assert!(e.contains("E0280"), "{e}");
        assert!(e.contains(&format!("{n}.ae:1")), "{e}");
        assert!(!e.contains("panicked"), "{e}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn symlink_loops_are_an_import_error() {
    let d = scratch("loop");
    std::os::unix::fs::symlink("b.ae", d.join("a.ae")).unwrap();
    std::os::unix::fs::symlink("a.ae", d.join("b.ae")).unwrap();
    let f = write(&d, "main.ae", "use \"a\";\nfn main() -> i32 { return 0; }\n");
    let o = run(&["check", &f]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("E0280"), "{}", err(&o));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_8_mib_limit_is_per_file_and_inclusive() {
    let d = scratch("big");
    let prog = "fn main() -> i32 { return 0; }\n";
    let limit = 8 * 1024 * 1024;
    let mut ok = String::from(prog);
    ok.push_str("//");
    ok.push_str(&"x".repeat(limit - ok.len() - 1));
    ok.push('\n');
    assert_eq!(ok.len(), limit);
    let f = write(&d, "ok.ae", &ok);
    assert!(run(&["check", &f]).status.success());
    ok.push('x');
    let f = write(&d, "big.ae", &ok);
    let o = run(&["check", &f]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("larger than 8 MiB"), "{}", err(&o));
    // the same limit applies to `use` and `--include`
    let main = write(&d, "main.ae", &format!("use {:?};\nfn main() -> i32 {{ return 0; }}\n", f));
    let o = run(&["check", &main]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("larger than 8 MiB"), "{}", err(&o));
    let o = run(&["check", &main, "--include", &f]);
    assert_eq!(o.status.code(), Some(1));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn very_long_identifiers_and_many_items_compile() {
    let d = scratch("long");
    let id = "a".repeat(50_000);
    let f = write(
        &d,
        "l.ae",
        &format!("fn {id}() -> i32 {{ return 7; }}\nfn main() -> i32 {{ print_i32({id}()); return 0; }}\n"),
    );
    assert_eq!(run_both(&f, &[]), "7\n");
    let mut src = String::new();
    for i in 0..5000 {
        src.push_str(&format!("fn g{i}() -> i32 {{ return {i}; }}\n"));
    }
    src.push_str("fn main() -> i32 { print_i32(g4999()); return 0; }\n");
    let f = write(&d, "many.ae", &src);
    assert_eq!(run_both(&f, &[]), "4999\n");
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// colour
// ---------------------------------------------------------------------------

#[test]
fn piped_stderr_has_no_ansi_escapes_unless_forced() {
    let d = scratch("color");
    let f = write(&d, "e.ae", "fn main() -> i32 { let x: i32 = \"a\"; return 0; }\n");
    for cmd in ["run", "check", "dump-ir", "compile", "verify", "stats"] {
        let o = run(&[cmd, &f]);
        assert_eq!(o.status.code(), Some(1), "{cmd}");
        assert!(!err(&o).contains('\u{1b}'), "{cmd}: {}", err(&o));
    }
    let o = run(&["check", &f, "--color"]);
    assert!(err(&o).contains('\u{1b}'), "{}", err(&o));
    let o = bin().args(["check", &f, "--no-color"]).output().unwrap();
    assert!(!err(&o).contains('\u{1b}'));
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// CLI front end
// ---------------------------------------------------------------------------

#[test]
fn malformed_options_are_errors_not_silent_defaults() {
    for (args, needle) in [
        (&["run", "examples/fib.ae", "-O9"][..], "invalid optimization level `9`"),
        (&["run", "examples/fib.ae", "-Ox"], "invalid optimization level `x`"),
        (&["run", "examples/fib.ae", "-O"], "-O needs a value"),
        (&["run", "examples/fib.ae", "examples/hello.ae"], "unexpected extra argument"),
        (&["run", "examples/fib.ae", "--max-steps", "0"], "--max-steps needs a positive integer"),
        (&["run", "examples/fib.ae", "--max-depth", "0"], "--max-depth needs a positive integer"),
        (&["run", "examples/fib.ae", "--max-steps"], "--max-steps needs"),
        (&["bench", "examples/fib.ae", "--n", "0"], "--n must be at least 1"),
        (&["bench", "examples/fib.ae", "--n", "x"], "--n needs an integer"),
        (&["compile", "examples/fib.ae", "--emit", "wasm"], "unknown --emit kind"),
        (&["compile", "examples/fib.ae", "-o"], "-o needs a value"),
        (&["fuzz", "--iters", "many"], "--iters needs"),
        (&["fuzz", "--seed", "zz"], "--seed needs"),
        (&["run", "examples/fib.ae", "--backend"], "--backend needs a value"),
    ] {
        let o = run(args);
        assert_eq!(o.status.code(), Some(1), "{args:?}: {}", err(&o));
        assert!(err(&o).contains(needle), "{args:?}: {}", err(&o));
    }
}

#[test]
fn limits_apply_to_run_profile_digest_and_bench_and_keep_partial_stdout() {
    let d = scratch("limits");
    let f = write(
        &d,
        "l.ae",
        "fn main() -> i32 { print_i32(1); let mut i = 0; while i < 100000 { i = i + 1; } return 0; }\n",
    );
    for cmd in ["run", "profile", "digest", "bench"] {
        let o = run(&[cmd, &f, "--max-steps", "1000"]);
        assert_eq!(o.status.code(), Some(2), "{cmd}: {}", err(&o));
        assert!(err(&o).contains("step limit"), "{cmd}: {}", err(&o));
        assert_eq!(out(&o), "1\n", "{cmd}");
    }
    let r = write(&d, "r.ae", "fn r(n: i32) -> i32 { return r(n + 1) + 1; }\nfn main() -> i32 { return r(0); }\n");
    for cmd in ["run", "profile", "digest", "bench"] {
        let o = run(&[cmd, &r, "--max-depth", "50"]);
        assert_eq!(o.status.code(), Some(2), "{cmd}");
        assert!(err(&o).contains("stack overflow"), "{cmd}: {}", err(&o));
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn profile_keeps_partial_stdout_on_runtime_errors() {
    let d = scratch("prof");
    let f = write(&d, "p.ae", "fn main() -> i32 { print_i32(1); let z = 0; print_i32(5 / z); return 0; }\n");
    let o = run(&["profile", &f]);
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(out(&o), "1\n");
    assert!(err(&o).contains("division by zero"), "{}", err(&o));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn llvm_backend_treats_mains_value_as_the_result() {
    if !path_has("lli") && !path_has("lli-18") {
        eprintln!("skipping: no lli on PATH");
        return;
    }
    let o = run(&["run", "examples/fib.ae", "--backend", "llvm"]);
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(out(&o).trim(), "55");
    let d = scratch("llvm");
    let f = write(&d, "r.ae", "fn main() -> i32 { print_i32(7); return 3; }\n");
    let o = run(&["run", &f, "--backend", "llvm", "--stats"]);
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(out(&o), "7\n");
    assert!(err(&o).contains("exit = 3"), "{}", err(&o));
    let f = write(&d, "n.ae", "fn main() -> i32 { return 0 - 1; }\n");
    let o = run(&["run", &f, "--backend", "llvm", "--stats"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(err(&o).contains("exit = 255"), "{}", err(&o));
    // an abort (failed assert) is a runtime error, like on the VM
    let f = write(&d, "a.ae", "fn main() -> i32 { assert(1 == 2); return 0; }\n");
    let o = run(&["run", &f, "--backend", "llvm"]);
    assert_eq!(o.status.code(), Some(2), "{}", err(&o));
    assert!(err(&o).contains("runtime error"), "{}", err(&o));
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// REPL
// ---------------------------------------------------------------------------

fn repl(input: &str) -> (String, String) {
    let mut child = bin()
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let o = child.wait_with_output().unwrap();
    assert!(o.status.success());
    (out(&o), err(&o))
}

#[test]
fn repl_keeps_definitions_across_errors_and_evaluates_expressions() {
    let (o, e) = repl(
        "fn sq(x: i32) -> i32 { return x * x; }\n\n\
         fn dbl(x: i32) -> i32 { return sq(x) + x; }\n\n\
         dbl(3)\n\n\
         fn sq(x: i32) -> i32 { return \"s\"; }\n\n\
         dbl(3)\n\n\
         fn sq(x: i32) -> i32 { return x * x * x; }\n\n\
         dbl(3)\n\n\
         print_i32(5)\n\n\
         :items\n:reset\n:items\n:quit\n",
    );
    assert!(o.contains("defined sq"), "{o}");
    assert!(o.contains("defined dbl"), "{o}");
    assert_eq!(o.matches("=> 12").count(), 2, "{o}");
    assert!(o.contains("(input discarded; previous definitions kept)"), "{o}");
    assert!(o.contains("=> 30"), "{o}");
    assert!(o.contains("5\n=> 0"), "{o}");
    assert!(o.contains("sq, dbl") || o.contains("dbl, sq"), "{o}");
    assert!(o.contains("(definitions cleared)") && o.contains("(no definitions)"), "{o}");
    assert!(e.contains("returning `string`"), "{e}");
    assert!(!e.contains('\u{1b}') && !o.contains('\u{1b}'));
}

#[test]
fn repl_defines_enums_and_names_that_merely_start_with_main() {
    let (o, e) = repl(
        "enum E { A, B }\n\n\
         fn main_loop() -> i32 { return 1; }\n\n\
         let e = E::A; match e { E::A => { print_i32(1); } E::B => { print_i32(2); } }\n\n\
         main_loop()\n\n\
         :items\n:quit\n",
    );
    assert!(o.contains("defined E"), "{o}\n{e}");
    assert!(o.contains("defined main_loop"), "{o}\n{e}");
    assert!(o.contains("1\n=> 0"), "{o}\n{e}");
    assert!(o.contains("=> 1"), "{o}\n{e}");
    assert!(o.contains("E, main_loop") || o.contains("main_loop, E"), "{o}\n{e}");
}

#[test]
fn repl_definitions_are_not_subject_to_pub_between_inputs() {
    let (o, e) = repl("struct P { x: i32, }\n\nfn get(p: P) -> i32 { return p.x; }\n\nget(P { x: 8 })\n\n:quit\n");
    assert!(o.contains("=> 8"), "{o}\n{e}");
}

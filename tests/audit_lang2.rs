//! Language gaps closed in the second language audit: zero-filled
//! uninitialised aggregates (E0232 for enums), `[value; N]` and empty
//! arrays, the `i64::MIN` literal, canonical tuple indices, end-of-file
//! parse errors, `pub enum` and file-private item names.
//!
//! Every runnable program is executed at -O0 and -O2 on the VM; both must
//! produce exactly the recorded stdout. Expected values were derived by hand
//! from `docs/language.md`.

use aether::{compile_source, compile_sources, run_compiled, CompileOptions, Compiled};

fn opts(opt_level: u8) -> CompileOptions {
    CompileOptions {
        opt_level,
        color: false,
    }
}

fn diags(c: &Compiled) -> String {
    c.diags.render(&c.session, false)
}

/// Runs `files` (the first is the main file) at -O0 and -O2 and returns
/// stdout, asserting both levels agree and succeed.
fn run_files(files: &[(&str, &str)]) -> String {
    let mut outs = Vec::new();
    for level in [0, 2] {
        let owned = files.iter().map(|(n, s)| (n.to_string(), s.to_string())).collect();
        let mut c = compile_sources(owned, &opts(level));
        assert!(!c.diags.has_errors(), "-O{level}:\n{}", diags(&c));
        let (_, out, _) = run_compiled(&mut c).unwrap_or_else(|e| panic!("-O{level}: {e}"));
        outs.push(out);
    }
    assert_eq!(outs[0], outs[1], "-O0 and -O2 differ");
    outs.remove(0)
}

fn run(src: &str) -> String {
    run_files(&[("audit.ae", src)])
}

/// Runtime error text at -O0 and -O2 (must agree).
fn run_err(src: &str) -> String {
    let mut errs = Vec::new();
    for level in [0, 2] {
        let mut c = compile_source("audit.ae", src, &opts(level));
        assert!(!c.diags.has_errors(), "{}", diags(&c));
        match run_compiled(&mut c) {
            Ok(_) => panic!("-O{level}: expected a runtime error"),
            Err(e) => errs.push(e.to_string()),
        }
    }
    assert_eq!(errs[0], errs[1]);
    errs.remove(0)
}

fn rejected_files(files: &[(&str, &str)]) -> String {
    let owned = files.iter().map(|(n, s)| (n.to_string(), s.to_string())).collect();
    let c = compile_sources(owned, &opts(0));
    assert!(c.diags.has_errors(), "expected an error for {files:?}");
    diags(&c)
}

fn rejected(src: &str) -> String {
    rejected_files(&[("audit.ae", src)])
}

// ---------------------------------------------------------------------------
// 1. uninitialised aggregates
// ---------------------------------------------------------------------------

#[test]
fn uninitialised_aggregates_are_zero_filled() {
    let src = r#"
struct P { x: i32, y: f64, s: string, b: bool, c: char }
struct Q { p: P, arr: [i64; 3], t: (i32, bool) }
fn main() -> i32 {
    let mut a: [i32; 3];
    print_i32(len(a));
    a[1] = 5;
    print_i32(a[0] + a[1] + a[2]);
    let s: P;
    print_i32(s.x); print_f64(s.y); print_i32(len(s.s)); print_bool(s.b); print_i32(s.c as i32);
    let mut q: Q;
    q.arr[2] = 7 as i64;
    print_i64(q.arr[0] + q.arr[2]);
    print_bool(q.t.1);
    let t: (i32, f64);
    print_f64(t.1 + 0.5);
    let mut big: [i64; 100000];
    big[99999] = 3 as i64;
    print_i64(big[99999] + big[5]);
    let mut nested: [[i32; 3]; 20];
    nested[4][2] = 9;
    print_i32(nested[4][2] + nested[5][2] + nested[3][2]);
    let e: [P; 2];
    print_i32(e[1].x);
    let z: [i32; 0];
    print_i32(len(z));
    return 0;
}
"#;
    assert_eq!(
        run(src),
        "3\n5\n0\n0\n0\nfalse\n0\n7\nfalse\n0.5\n3\n9\n0\n0\n"
    );
}

#[test]
fn uninitialised_values_are_fresh_each_iteration_and_independent() {
    let src = r#"
struct G { cells: [[bool; 4]; 4], score: (i64, f64), name: string }
fn bump(g: G) -> G { let mut h = g; h.cells[3][3] = true; h.score.0 = h.score.0 + 1 as i64; return h; }
fn main() -> i32 {
    let mut total = 0;
    for i in 0..3 {
        let mut a: [i32; 3];
        a[i] = a[i] + 1 + i;
        total = total + a[0] + a[1] + a[2];
        let mut n: i32;
        n += 1;
        total += n;
    }
    print_i32(total);
    let g: G;
    let h = bump(g);
    print_bool(g.cells[3][3]);
    print_bool(h.cells[3][3]);
    print_i64(h.score.0);
    print_bool(g == g);
    print_bool(h != g);
    let x: i64;
    {
        let x: [i64; 2];
        print_i64(x[1]);
    }
    print_i64(x);
    return 0;
}
"#;
    assert_eq!(run(src), "9\nfalse\ntrue\n1\ntrue\ntrue\n0\n0\n");
}

#[test]
fn enum_variables_need_an_initializer() {
    let e = rejected("enum E { A, B } fn main() -> i32 { let e: E; return 0; }");
    assert!(e.contains("E0232") && e.contains("enum variable `e` needs an initializer"), "{e}");
    let e = rejected(
        "enum E { A } struct S { e: E } fn main() -> i32 { let s: S; let t: (i32, [S; 2]); return 0; }",
    );
    assert_eq!(e.matches("E0232").count(), 2, "{e}");
    assert!(e.contains("contains the enum `E`"), "{e}");
    // an empty array of enums needs no element
    assert_eq!(run("enum E { A } fn main() -> i32 { let z: [E; 0]; print_i32(len(z)); return 0; }"), "0\n");
}

// ---------------------------------------------------------------------------
// 2. `[value; N]` and empty arrays
// ---------------------------------------------------------------------------

#[test]
fn array_repeat_copies_one_evaluation() {
    let src = r#"
struct P { x: i32 }
enum E { A(i32), B }
fn tick(n: i32) -> i32 { print_i32(n); return n; }
fn main() -> i32 {
    let mut m = [[0; 3]; 2];
    m[0][1] = 5;
    print_i32(m[1][1] + m[0][1]);
    let mut ps = [P { x: 1 }; 10];
    ps[9].x = 4;
    print_i32(ps[0].x + ps[9].x);
    let a = [tick(1); 0];
    let b = [tick(2); 1];
    let c = [[tick(3); 2]; 2];
    let _d = [tick(4); 100];
    print_i32(len(a) + len(b) + len(c) + c[1][1]);
    let w: [i64; 3] = [1; 3];
    print_i64(w[2]);
    let e = E::A(3);
    let v = [match e { E::A(n) => n, E::B => 0 }; 9];
    print_i32(v[8]);
    print_bool([E::B; 3][2] == E::B);
    let mut big = [1 as i64; 100000];
    big[0] = 5 as i64;
    print_i64(big[0] + big[99999]);
    let mut s = 0;
    for i in 0..3 {
        let r = [i; 20];
        s += r[19] + len(r);
    }
    print_i32(s);
    return 0;
}
"#;
    assert_eq!(run(src), "5\n5\n1\n2\n3\n4\n6\n1\n3\ntrue\n6\n63\n");
}

#[test]
fn array_repeat_of_a_large_count_lowers_to_a_loop() {
    let src = "fn main() -> i32 { let a = [7; 100000]; let b: [i32; 50000]; return a[99999] + b[1]; }";
    for level in [0, 2] {
        let c = compile_source("audit.ae", src, &opts(level));
        assert!(!c.diags.has_errors(), "{}", diags(&c));
        let ir = c.ir.as_ref().expect("ir");
        let insts: usize = ir.functions.iter().flat_map(|f| &f.blocks).map(|b| b.insts.len()).sum();
        assert!(insts < 100, "-O{level}: {insts} instructions");
    }
    assert!(run_err("fn main() -> i32 { let a = [5; 3]; return a[3]; }").contains("out of bounds"));
}

#[test]
fn empty_arrays_take_their_type_from_the_context() {
    let src = r#"
struct W { a: [i32; 0], b: i32 }
fn f(x: [string; 0]) -> [f64; 0] { print_i32(len(x)); return []; }
fn main() -> i32 {
    let w = W { a: [], b: 1 };
    let e: [[i32; 0]; 2] = [[], []];
    print_i32(len(e) + len(e[1]) + w.b);
    let r = f([]);
    print_i32(len(r));
    let t: ([i32; 0], i32) = ([], 5);
    print_i32(t.1);
    print_bool(w.a == []);
    let q: [i32; 0];
    print_bool(q == w.a);
    let m: [i32; 0] = match len(q) { 0 => [], _ => [] };
    print_i32(len(m));
    let mut z: [i32; 0] = [];
    z = [];
    print_i32(len(z));
    return 0;
}
"#;
    assert_eq!(run(src), "3\n0\n0\n5\ntrue\ntrue\n0\n0\n");
    let e = run_err("fn main() -> i32 { let z: [i32; 0] = []; let i = 0; return z[i]; }");
    assert!(e.contains("array index 0 out of bounds"), "{e}");
    let e = rejected("fn main() -> i32 { let z = []; return 0; }");
    assert!(e.contains("E0250"), "{e}");
    let e = rejected("fn main() -> i32 { let a: [i32; 3] = []; return 0; }");
    assert!(e.contains("cannot assign `[i32; 0]` to variable of type `[i32; 3]`"), "{e}");
}

#[test]
fn array_repeat_syntax_errors() {
    for (src, want) in [
        ("fn main() -> i32 { let a = [1; ]; return 0; }", "expected integer literal, found `]`"),
        ("fn main() -> i32 { let a = [1; x]; return 0; }", "expected integer literal, found `x`"),
        ("fn main() -> i32 { let a = [1, 2; 3]; return 0; }", "expected `]`, found `;`"),
        ("fn main() -> i32 { let a = [1; 99999999999999999999]; return 0; }", "out of range for i64"),
        ("fn main() -> i32 { let a = [1; 3000000000]; return 0; }", "E0262"),
        ("fn main() -> i32 { let a: [i32; 3000000000]; return 0; }", "E0262"),
        ("fn main() -> i32 { let a: [i32; 99999999999999999999]; return 0; }", "out of range for i64"),
        ("fn main() -> i32 { let a: [i32; 2] = [1; 3]; return 0; }", "E0230"),
    ] {
        let e = rejected(src);
        assert!(e.contains(want), "{src}: {e}");
    }
}

// ---------------------------------------------------------------------------
// 3. `i64::MIN`
// ---------------------------------------------------------------------------

#[test]
fn i64_min_literal() {
    let src = r#"
fn f(x: i64) -> i32 {
    match x {
        -9223372036854775808 => { return 1; }
        9223372036854775807 => { return 2; }
        _ => { return 4; }
    }
}
fn main() -> i32 {
    let a: i64 = -9223372036854775808;
    print_i64(a);
    print_bool(a == 9223372036854775807 + 1 as i64);
    print_i64(-0x8000_0000_0000_0000);
    let arr: [i64; 2] = [-9223372036854775808; 2];
    print_i64(arr[1]);
    let t: (i64, i32) = (-9223372036854775808, -2147483648);
    print_i64(t.0);
    print_i32(t.1);
    print_i32(f(-9223372036854775807 - 1 as i64));
    print_i32(f(a + 1 as i64));
    let b: i64 = -9223372036854775808 + 1;
    print_i64(b);
    return 0;
}
"#;
    assert_eq!(
        run(src),
        "-9223372036854775808\ntrue\n-9223372036854775808\n-9223372036854775808\n-9223372036854775808\n-2147483648\n1\n4\n-9223372036854775807\n"
    );
    for (src, want) in [
        ("fn main() -> i32 { let x: i32 = -9223372036854775808; return 0; }", "E0263"),
        ("fn main() -> i32 { let x = -9223372036854775808; return 0; }", "E0263"),
        ("fn main() -> i32 { let x: i64 = 9223372036854775808; return 0; }", "integer literal out of range for i64"),
        ("fn main() -> i32 { let x: i64 = 0 - 9223372036854775808; return 0; }", "integer literal out of range for i64"),
        ("fn main() -> i32 { print_i64(--9223372036854775808); return 0; }", "`9223372036854775808` is out of range for `i64`"),
        ("fn main() -> i32 { let x: i64 = -9223372036854775809; return 0; }", "integer literal out of range for i64"),
        ("fn main() -> i32 { return 0x; }", "invalid integer literal"),
    ] {
        let e = rejected(src);
        assert!(e.contains(want), "{src}: {e}");
    }
}

// ---------------------------------------------------------------------------
// 4. tuple indices
// ---------------------------------------------------------------------------

#[test]
fn tuple_indices_are_canonical_decimals() {
    assert_eq!(
        run("fn main() -> i32 { let t = (0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11); print_i32(t.10 + t.11); \
             let n = ((1, 2), (3, (4, 5))); print_i32(n.1.1.0 + n.0.1); return 0; }"),
        "21\n6\n"
    );
    for idx in ["01", "1.01", "00.1", "0x1", "1e5", "1_0"] {
        let src = format!("fn main() -> i32 {{ let t = ((1, 2), 3); return t.{idx}; }}");
        let e = rejected(&src);
        assert!(e.contains("invalid tuple index"), "{src}: {e}");
    }
}

// ---------------------------------------------------------------------------
// 5. errors at the end of the file
// ---------------------------------------------------------------------------

#[test]
fn end_of_file_is_named_in_parse_errors() {
    let e = rejected("use \"");
    assert!(e.contains("unterminated string literal"), "{e}");
    assert!(e.contains("expected `;`, found end of file"), "{e}");
    for (src, want) in [
        ("fn main() -> i32 { return 1", "expected `;`, found end of file"),
        ("struct", "expected identifier, found end of file"),
        ("fn main(", "expected identifier, found end of file"),
        ("enum E { A(", "expected `)`, found end of file"),
        ("pub", "expected item, found end of file"),
        ("fn main() -> i32 { let t = (1, 2); return t.", "expected identifier, found end of file"),
        ("fn main() -> i32 { return 1 }", "expected `;`, found `}`"),
    ] {
        let e = rejected(src);
        assert!(e.contains(want), "{src}: {e}");
        assert!(!e.contains("found ``"), "{src}: {e}");
    }
    // unwinding out of nested blocks reports the missing `}` once
    let e = rejected("fn main() -> i32 { if true { while false { ");
    assert_eq!(e.matches("expected `}`, found end of file").count(), 1, "{e}");
}

// ---------------------------------------------------------------------------
// 6. `pub enum`
// ---------------------------------------------------------------------------

const ENUM_LIB: &str = "\
enum Color { Red, Green }
pub enum Shape { Circle(i32), Sq(i32) }
pub fn mk() -> Color { return Color::Green; }
pub fn same(a: Color, b: Color) -> bool { return a == b; }
";

#[test]
fn pub_enums_and_their_variants_are_visible_to_other_files() {
    let main = "fn main() -> i32 {
    let s = Shape::Sq(3);
    match s { Shape::Circle(r) => { print_i32(r); } Shape::Sq(n) => { print_i32(n * 2); } }
    let c = mk();
    print_bool(same(c, mk()));
    return 0;
}";
    assert_eq!(run_files(&[("main.ae", main), ("lib.ae", ENUM_LIB)]), "6\ntrue\n");
}

#[test]
fn private_enums_are_e0281_in_other_files() {
    for main in [
        "fn main() -> i32 { let c = Color::Red; return 0; }",
        "fn f(c: Color) -> i32 { return 0; } fn main() -> i32 { return 0; }",
        "fn main() -> i32 { match mk() { Color::Red => { return 1; } _ => { return 0; } } }",
    ] {
        let e = rejected_files(&[("main.ae", main), ("lib.ae", ENUM_LIB)]);
        assert!(e.contains("E0281") && e.contains("`Color` is private to `lib.ae`"), "{main}: {e}");
        assert!(e.contains("enum `Color` is defined at lib.ae:1:6 without `pub`"), "{e}");
    }
}

// ---------------------------------------------------------------------------
// 7. file-private names
// ---------------------------------------------------------------------------

const A: &str = "\
fn helper() -> i32 { return 10; }
struct S { v: i32 }
enum K { One }
pub fn from_a() -> i32 { let s = S { v: 1 }; match K::One { K::One => { return helper() + s.v; } } }
fn len(x: i32) -> i32 { return 100 + x; }
pub fn ua() -> i32 { return len(1); }
";
const B: &str = "\
fn helper() -> i32 { return 20; }
struct S { w: f64, v: i32 }
enum K { Two, Three }
pub fn from_b() -> i32 { let s = S { w: 1.0, v: 2 }; match K::Three { K::Three => { return helper() + s.v; } _ => { return 0; } } }
";

#[test]
fn private_items_of_different_files_do_not_clash() {
    let main = "fn helper() -> i32 { return 30; }
struct S { name: string }
fn main() -> i32 {
    print_i32(from_a());
    print_i32(from_b());
    print_i32(helper());
    let s = S { name: \"abc\" };
    print_i32(len(s.name));
    print_i32(ua());
    return 0;
}";
    assert_eq!(
        run_files(&[("main.ae", main), ("a.ae", A), ("b.ae", B)]),
        "11\n22\n30\n3\n101\n"
    );
    // dumps print the source name
    let c = compile_sources(
        vec![("main.ae".into(), main.into()), ("a.ae".into(), A.into()), ("b.ae".into(), B.into())],
        &opts(0),
    );
    let ir = aether::ir::dump_ir(c.ir.as_ref().expect("ir"));
    assert!(ir.contains("fn helper()") && !ir.contains('$'), "{ir}");
}

#[test]
fn pub_names_still_clash_and_private_names_stay_private() {
    // a `pub` item clashes with any item of the same name
    let e = rejected_files(&[
        ("main.ae", "fn main() -> i32 { return helper(); }"),
        ("a.ae", A),
        ("c.ae", "pub fn helper() -> i32 { return 1; }"),
    ]);
    assert!(e.contains("E0203") && e.contains("duplicate function `helper`"), "{e}");
    let e = rejected_files(&[
        ("main.ae", "pub struct S { q: i32 } fn main() -> i32 { return 0; }"),
        ("a.ae", A),
    ]);
    assert!(e.contains("E0201") && e.contains("duplicate struct `S`"), "{e}");
    // two items of one file clash as before
    let e = rejected_files(&[
        ("main.ae", "fn main() -> i32 { return 0; }"),
        ("a.ae", "fn h() -> i32 { return 1; } fn h() -> i32 { return 2; }"),
    ]);
    assert!(e.contains("duplicate function `h`"), "{e}");
    // `main` is never file-private
    let e = rejected_files(&[
        ("main.ae", "fn main() -> i32 { return 0; }"),
        ("a.ae", "fn main() -> i32 { return 1; }"),
    ]);
    assert!(e.contains("duplicate function `main`"), "{e}");
    // another file's private item is still E0281 when nothing visible has the name
    let e = rejected_files(&[("main.ae", "fn main() -> i32 { return helper(); }"), ("a.ae", A)]);
    assert!(e.contains("E0281") && e.contains("`helper` is private to `a.ae`"), "{e}");
    let e = rejected_files(&[("main.ae", "fn main() -> i32 { let s: S; return 0; }"), ("a.ae", A)]);
    assert!(e.contains("E0281") && e.contains("struct `S`"), "{e}");
    // a private item shadows nothing across files: `len` in a.ae is a.ae's own
    assert_eq!(
        run_files(&[("main.ae", "fn main() -> i32 { print_i32(len(\"xy\") + ua()); return 0; }"), ("a.ae", A)]),
        "103\n"
    );
}

#[test]
fn extern_fns_keep_their_host_name() {
    // an extern's name is the host symbol, so it is never renamed
    let e = rejected_files(&[
        ("main.ae", "extern fn host(x: i32) -> i32; fn main() -> i32 { return 0; }"),
        ("a.ae", "extern fn host(x: i32) -> i32;"),
    ]);
    assert!(e.contains("duplicate function `host`"), "{e}");
    let c = compile_sources(
        vec![
            ("main.ae".into(), "fn main() -> i32 { return call_host(); }".into()),
            ("a.ae".into(), "extern fn host(x: i32) -> i32; pub fn call_host() -> i32 { return host(1); }".into()),
        ],
        &opts(0),
    );
    assert!(!c.diags.has_errors(), "{}", diags(&c));
    let ir = c.ir.as_ref().expect("ir");
    assert!(ir.functions.iter().any(|f| f.name == "host" && f.is_extern));
}

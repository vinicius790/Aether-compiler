//! Language-semantics and `fmt` audit (A1): postfix operators after
//! literals, struct literals and `match` expressions, field-less struct
//! literals, duplicate struct-literal fields and parameters, `i32`-only
//! indices, `'ab'` and match-arm diagnostics, and `fmt` printing only the
//! parentheses the grammar needs (so its output stays under the nesting
//! limit), compound assignments, `if let` / `else if` chains and `()` returns;
//! long operator chains are bounded (E0101) instead of overflowing the stack.
//!
//! Every runnable program is executed at -O0 and -O2 on the VM; both must
//! produce exactly the recorded stdout. Expected values were derived by hand
//! from `docs/language.md`.

use aether::comments::format_source;
use aether::{compile_source, run_compiled, CompileOptions, Compiled};

fn opts(opt_level: u8) -> CompileOptions {
    CompileOptions {
        opt_level,
        color: false,
    }
}

fn diags(c: &Compiled) -> String {
    c.diags.render(&c.session, false)
}

/// stdout at -O0 and -O2 (must agree and succeed).
fn run(src: &str) -> String {
    let mut outs = Vec::new();
    for level in [0, 2] {
        let mut c = compile_source("a1.ae", src, &opts(level));
        assert!(!c.diags.has_errors(), "-O{level}:\n{}", diags(&c));
        let (_, out, _) = run_compiled(&mut c).unwrap_or_else(|e| panic!("-O{level}: {e}"));
        outs.push(out);
    }
    assert_eq!(outs[0], outs[1], "-O0 and -O2 differ");
    outs.remove(0)
}

/// All diagnostics of a program that must not compile.
fn rejected(src: &str) -> String {
    let c = compile_source("a1.ae", src, &opts(0));
    assert!(c.diags.has_errors(), "expected an error for {src}");
    diags(&c)
}

fn error_count(text: &str) -> usize {
    text.lines().filter(|l| l.starts_with("error")).count()
}

/// `fmt` output, checked to be a fixpoint and to run like the source.
fn fmt_same(src: &str) -> String {
    let once = format_source(src).expect("fmt");
    let twice = format_source(&once).expect("fmt of fmt");
    assert_eq!(once, twice, "fmt is not idempotent:\n{once}");
    assert_eq!(run(&once), run(src), "fmt changed the meaning:\n{once}");
    once
}

#[test]
fn postfix_after_literals_struct_literals_and_match() {
    let src = r#"
struct S { a: i32 }
fn main() -> i32 {
    print_i32(S { a: 9 }.a + (S { a: 1 }).a);
    print_char("ação"[2]);
    let t = match 1 { 0 => (1, 2), _ => (3, 4) }.1;
    print_i32(t);
    print_i32(match 2 { _ => [5, 6] }[1] * 2);
    return 0;
}
"#;
    assert_eq!(run(src), "10\nã\n4\n12\n");
    fmt_same(src);
    // a statement `match` still ends at its `}`
    let e = rejected("fn main() -> i32 { match 1 { _ => 3 } + 1; return 0; }");
    assert!(e.contains("expected expression, found `+`"), "{e}");
}

#[test]
fn fieldless_struct_literal() {
    let src = r#"
struct U {}
struct W { u: U, n: i32 }
fn mk() -> U { return U {}; }
fn main() -> i32 {
    let u = U {};
    let w: U;
    print_bool(u == w);
    if (u == U {}) { print_i32(2); }
    while u != (U {}) { print_i32(3); }
    match (U {}, 1) { (_, n) => print_i32(n) }
    let x = W { u: U {}, n: 4 };
    print_bool(mk() == x.u);
    let arr = [U {}, mk()];
    print_i32(len(arr) + x.n);
    if match 1 { _ => U {} } == u { print_i32(5); }
    return 0;
}
"#;
    assert_eq!(run(src), "true\n2\n1\ntrue\n6\n5\n");
    let f = fmt_same(src);
    assert!(f.contains("let u = U {};"), "{f}");
    assert!(f.contains("if (u == U {}) {"), "{f}");
    assert!(f.contains("while (u != U {}) {"), "{f}");
    // in a head `U {}` is `U` and a block, as in Rust; the error says so
    let e = rejected("struct U {}\nfn main() -> i32 { let u = U {}; if u == U {} { } return 0; }");
    assert!(e.contains("cannot find value `U`"), "{e}");
    assert!(e.contains("write the literal in parentheses: `(U {})`"), "{e}");
}

#[test]
fn duplicate_fields_and_parameters() {
    let e = rejected("struct S { a: i32 } fn main() -> i32 { let s = S { a: 1, a: 2 }; return s.a; }");
    assert!(e.contains("[E0275] field `a` is specified more than once in `S` literal"), "{e}");
    let e = rejected("fn f(a: i32, a: i32) -> i32 { return a; } fn main() -> i32 { return f(1, 2); }");
    assert!(e.contains("[E0274] `a` is bound more than once in the parameter list"), "{e}");
    // a repeated pattern name is E0274 only, not also a shadowing warning
    let e = rejected("fn main() -> i32 { let (a, a) = (1, 2); return a; }");
    assert!(e.contains("E0274") && !e.contains("W0232"), "{e}");
}

#[test]
fn indices_are_i32() {
    // an `i64` index used to type-check and then fail on every backend
    for src in [
        "fn main() -> i32 { let a = [1, 2]; let i: i64 = 1; return a[i]; }",
        "fn main() -> i32 { let s = \"ab\"; let i: i64 = 0; print_char(s[i]); return 0; }",
        "fn main() -> i32 { let a = [[1]]; let i: i64 = 0; a[i][0] = 2; return 0; }",
    ] {
        let e = rejected(src);
        assert!(e.contains("[E0246] index must be `i32`, found `i64`"), "{e}");
    }
    let e = rejected("fn main() -> i32 { let a = [1, 2]; return a[true]; }");
    assert!(e.contains("index must be `i32`, found `bool`"), "{e}");
    assert_eq!(
        run("fn main() -> i32 { let a = [1, 2]; let i: i64 = 1; print_i32(a[i as i32]); return 0; }"),
        "2\n"
    );
}

#[test]
fn one_diagnostic_per_mistake() {
    let e = rejected("fn main() -> i32 { let c = 'ab'; return 0; }");
    assert!(e.contains("character literal may only contain one character"), "{e}");
    assert_eq!(error_count(&e), 1, "{e}");
    let e = rejected("fn main() -> i32 { let c = 'a; return 0; }");
    assert!(e.contains("unterminated character literal"), "{e}");
    // match arms: or-patterns, guards and assignments get a help, and the
    // rest of the match is skipped instead of cascading into item errors
    for (src, help) in [
        ("match x { 1 | 2 => print_i32(1), _ => {} }", "or-patterns are not supported"),
        ("match x { n if n > 2 => print_i32(1), _ => {} }", "match guards are not supported"),
        ("match x { 1 => y += 1, _ => {} }", "an assignment is a statement"),
    ] {
        let prog = format!(
            "fn main() -> i32 {{ let x = 3; let mut y = 0; {src} return y; }}\nfn g() -> i32 {{ return 1; }}"
        );
        let e = rejected(&prog);
        assert!(e.contains(help), "{e}");
        assert_eq!(error_count(&e), 1, "{e}");
    }
}

#[test]
fn for_variable_assignment_affects_iterations() {
    assert_eq!(
        run("fn main() -> i32 { for i in 0..5 { print_i32(i); i += 2; } return 0; }"),
        "0\n3\n"
    );
}

#[test]
fn fmt_prints_only_needed_parentheses() {
    let src = r#"
fn main() -> i32 {
    let mut x = 3;
    x = x - (1 - 4);
    x = (x) * 2;
    x = -(5) - x;
    x = x - -x;
    print_i32(x);
    print_i32(-(x as i64) as i32);
    print_i32((-x) as i32);
    print_bool(!(x < 0) == (x >= 0));
    print_i32((1 + 2) * (3 - (4 - 5)) % 5);
    print_i32(1 - (2 - 3) - (4 + 5));
    print_i32(1 << (2 << 1) >> 1);
    print_i32((match x { 0 => 1, _ => 2 }) as i64 as i32);
    let m: i64 = -9223372036854775808;
    print_i64(--9223372036854775807 + m);
    return 0;
}
"#;
    assert_eq!(run(src), "-34\n34\n34\ntrue\n2\n-7\n8\n2\n-1\n");
    let f = fmt_same(src);
    for line in [
        "x -= 1 - 4;",
        "x *= 2;",
        "x = -(5) - x;",
        "x -= -x;",
        "print_i32(-(x as i64) as i32);",
        "print_i32(-x as i32);",
        "print_i32((1 + 2) * (3 - (4 - 5)) % 5);",
        "print_i32(1 - (2 - 3) - (4 + 5));",
        "print_i32(1 << (2 << 1) >> 1);",
    ] {
        assert!(f.contains(line), "missing `{line}` in:\n{f}");
    }
}

#[test]
fn fmt_output_stays_under_the_nesting_limit() {
    // 300 left-associative additions are flat in the source; printed with a
    // pair of parentheses per operator they nested 300 deep (E0101)
    let sum = vec!["1"; 300].join(" + ");
    let src = format!("fn main() -> i32 {{ print_i32({sum}); return 0; }}");
    assert_eq!(run(&src), "300\n");
    fmt_same(&src);
    // an `else if` chain is one level, not one block per link
    let mut chain = String::from("fn f(x: i32) -> i32 { if x == 0 { return 0; }");
    for i in 1..150 {
        chain.push_str(&format!(" else if x == {i} {{ return {i}; }}"));
    }
    chain.push_str(" return -1; }\nfn main() -> i32 { print_i32(f(149)); print_i32(f(500)); return 0; }");
    assert_eq!(run(&chain), "149\n-1\n");
    let f = fmt_same(&chain);
    assert!(f.contains("} else if x == 2 {"), "{f}");
}

#[test]
fn fmt_keeps_statement_forms() {
    let src = r#"
enum E { A(i32), B(bool), C }
fn u(e: E) {
    match e { E::A(n) => print_i32(n), _ => {} }
}
fn f(e: E) -> i32 {
    if let E::A(n) = e {
        return n;
    } else if let E::B(true) = e {
        return 100;
    } else if e == E::C {
        return 200;
    }
    if let E::B(_) = e { return 300; }
    return -1;
}
fn main() -> i32 {
    u(E::A(7));
    print_i32(f(E::A(5)) + f(E::B(true)) + f(E::C) + f(E::B(false)));
    {
        match 1 { _ => print_i32(1) };
    }
    return 0;
}
"#;
    assert_eq!(run(src), "7\n605\n1\n");
    let f = fmt_same(src);
    assert!(f.contains("fn u(e: E) {"), "{f}");
    assert!(f.contains("} else if let E::B(true) = e {"), "{f}");
    assert!(f.contains("    if let E::B(_) = e {\n"), "{f}");
    // a `match` last in a block keeps its `;` so it stays a statement
    assert!(f.contains("        };\n    }\n    return 0;"), "{f}");
}

#[test]
fn operator_chains_are_bounded() {
    // a 100000-term `x + x + ...` overflowed the CLI's 64 MiB stack; the
    // chain is now limited (E0101), the limit itself still compiles
    let worker = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let chain = |n: usize| {
                format!(
                    "fn main() -> i32 {{ let x = 1; print_i32({}); return 0; }}",
                    vec!["x"; n].join(" + ")
                )
            };
            let max = aether::parser::MAX_CHAIN;
            assert_eq!(run(&chain(max + 1)), format!("{}\n", max + 1));
            for n in [max + 2, 100_000] {
                let e = rejected(&chain(n));
                assert!(e.contains("[E0101] operator chain too long (limit 10000 operators)"), "{e}");
                assert_eq!(error_count(&e), 1, "{e}");
            }
        })
        .expect("spawn");
    worker.join().expect("chain test");
}

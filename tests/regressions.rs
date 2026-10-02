//! One regression test per bug found in the 0.2.1 audit. Every program is
//! executed at both -O0 and -O2; the two levels must agree with each other
//! and with the expected value and stdout.

use aether::{run_source, Value};

fn run(src: &str, level: u8) -> Result<(Value, String), String> {
    run_source("regress.ae", src, level).map(|(v, out, _)| (v, out))
}

/// Runs at -O0 and -O2, asserts both agree and match `value` / `stdout`.
fn check(src: &str, value: i32, stdout: &str) {
    let o0 = run(src, 0).unwrap_or_else(|e| panic!("-O0 failed:\n{e}\n{src}"));
    let o2 = run(src, 2).unwrap_or_else(|e| panic!("-O2 failed:\n{e}\n{src}"));
    assert_eq!(o0, o2, "-O0 and -O2 disagree\n{src}");
    assert_eq!(o0.0, Value::I32(value), "{src}");
    assert_eq!(o0.1, stdout, "{src}");
}

fn rejected(src: &str) -> String {
    match run(src, 0) {
        Err(e) => e,
        Ok(v) => panic!("expected a compile error, got {v:?}\n{src}"),
    }
}

// ---------- miscompilations ----------

#[test]
fn tail_expression_is_the_return_value() {
    check("fn f() -> i32 { 42 }\nfn main() -> i32 { f() }", 42, "");
    check(
        "fn f(x: i32) -> i32 { let y = x * 2; y + 1 }\nfn main() -> i32 { return f(20); }",
        41,
        "",
    );
}

#[test]
fn nested_block_tail_is_a_statement() {
    // `if c { 5 }` does not return from the function
    check(
        "fn main() -> i32 { let c = true; if c { print_i32(5) } return 7; }",
        7,
        "5\n",
    );
}

#[test]
fn struct_literal_fields_in_any_order() {
    check(
        "struct P { x: i32, y: i32 }\nfn main() -> i32 { let p = P { y: 2, x: 1 }; print_i32(p.x); print_i32(p.y); return p.x * 10 + p.y; }",
        12,
        "1\n2\n",
    );
}

#[test]
fn nested_assignment_reaches_the_root() {
    let src = r#"
        struct In { v: i32 }
        struct Out { i: In, k: i32 }
        fn main() -> i32 {
            let mut o = Out { i: In { v: 1 }, k: 0 };
            o.i.v = 99;
            let mut a = [[1, 2], [3, 4]];
            a[0][1] = 77;
            a[1][0] = a[0][1] + 1;
            print_i32(o.i.v);
            print_i32(a[0][1]);
            print_i32(a[1][0]);
            return 0;
        }
    "#;
    check(src, 0, "99\n77\n78\n");
}

#[test]
fn for_variable_is_scoped_to_the_loop() {
    check(
        "fn main() -> i32 { let i = 5; for i in 0..3 { print_i32(i); } return i; }",
        5,
        "0\n1\n2\n",
    );
}

#[test]
fn logical_operators_short_circuit() {
    let src = r#"
        fn boom() -> bool { print_i32(-1); return true; }
        fn main() -> i32 {
            let a = [1, 2, 3];
            let i = 5;
            let mut hits = 0;
            if i < 3 && a[i] > 0 { hits = hits + 1; }
            if i > 3 || a[i] > 0 { hits = hits + 10; }
            if false && boom() { hits = hits + 100; }
            if true || boom() { hits = hits + 1000; }
            if i < 3 || a[1] == 2 { hits = hits + 10000; }
            return hits;
        }
    "#;
    check(src, 11010, "");
}

#[test]
fn array_copy_has_value_semantics() {
    let src = r#"
        struct P { x: i32 }
        fn bump(p: P) -> i32 { let mut c = p; c.x = c.x + 1; return c.x; }
        fn main() -> i32 {
            let a = [1, 2, 3];
            let mut b = a;
            b[0] = 9;
            let p = P { x: 1 };
            let q = p;
            let mut r = q;
            r.x = 5;
            print_i32(a[0]);
            print_i32(b[0]);
            print_i32(bump(p));
            print_i32(p.x);
            print_i32(q.x);
            print_i32(r.x);
            return 0;
        }
    "#;
    check(src, 0, "1\n9\n2\n1\n1\n5\n");
}

#[test]
fn nan_is_not_folded_by_algebraic_identities() {
    let src = r#"
        fn main() -> i32 {
            let z: f64 = 0.0;
            let n = 1.0 / z;
            let m = n - n;
            print_bool(m == m);
            print_bool(n * 0.0 == 0.0);
            let i: i64 = 7;
            print_i64(i - i);
            print_bool(i == i);
            return 0;
        }
    "#;
    check(src, 0, "false\nfalse\n0\ntrue\n");
}

#[test]
fn more_than_255_registers() {
    let mut src = String::from("fn main() -> i32 {\n  let mut t = 0;\n");
    for i in 0..300 {
        src.push_str(&format!("  let v{i} = {i} + 1;\n"));
    }
    for i in 0..300 {
        src.push_str(&format!("  t = t + v{i};\n"));
    }
    src.push_str("  return t;\n}\n");
    check(&src, 45150, "");
}

#[test]
fn out_of_range_i32_literal_is_rejected() {
    let e = rejected("fn main() -> i32 { let x = 3000000000; return 0; }");
    assert!(e.contains("out of range"), "{e}");
    let e = rejected("fn main() -> i32 { return -2147483649; }");
    assert!(e.contains("out of range"), "{e}");
}

#[test]
fn every_accepted_comparison_executes() {
    let src = r#"
        fn main() -> i32 {
            let s = "abc";
            print_bool(s == "abc");
            print_bool(s != "abc" + "d");
            let c = 'a';
            print_bool(c == 'a');
            print_bool(c < 'b');
            print_bool(c >= 'b');
            print_bool(true != false);
            let x: i64 = 7;
            let y: i64 = 3;
            print_i64(x % y);
            print_bool(x != y);
            print_bool(x >= y);
            print_bool(x <= y);
            print_bool(x > y);
            let f = 1.5;
            print_bool(f != 1.5);
            print_bool(f <= 2.0);
            print_bool(f > 2.0);
            print_bool(f >= 1.5);
            return 0;
        }
    "#;
    check(
        src,
        0,
        "true\ntrue\ntrue\ntrue\nfalse\ntrue\n1\ntrue\ntrue\nfalse\ntrue\nfalse\ntrue\nfalse\ntrue\n",
    );
}

#[test]
fn i64_negation_keeps_64_bits() {
    check(
        "fn main() -> i32 { let a: i64 = 5000000000; let b = -a; print_i64(b); print_i64(-(a + a)); return 0; }",
        0,
        "-5000000000\n-10000000000\n",
    );
}

#[test]
fn casts_execute() {
    let src = r#"
        fn main() -> i32 {
            let x: i64 = 3;
            print_f64(x as f64);
            print_i64(2.9 as i64);
            print_i32('A' as i32);
            print_i32((66 as char) as i32);
            print_i64(true as i64);
            print_i32(false as i32);
            print_i64(7 as i64);
            print_i32(x as i32);
            return 0;
        }
    "#;
    check(src, 0, "3\n2\n65\n66\n1\n0\n7\n3\n");
}

#[test]
fn unbound_extern_is_a_runtime_error_not_print() {
    let src = "extern fn foo(x: string);\nfn main() -> i32 { foo(\"hello\"); return 0; }";
    for level in [0, 2] {
        let e = run(src, level).expect_err("extern call must fail");
        assert!(e.contains("extern"), "{e}");
        assert!(!e.contains("hello"), "must not behave like print: {e}");
    }
}

// ---------- crashes and usability ----------

#[test]
fn min_div_minus_one_wraps_instead_of_panicking() {
    check(
        "fn main() -> i32 { let a = 0 - 2147483647 - 1; let b = 0 - 1; print_i32(a / b); print_i32(a % b); let c = a; let d = b; return c / d; }",
        -2147483648,
        "-2147483648\n0\n",
    );
}

#[test]
fn deep_nesting_is_diagnosed_not_a_crash() {
    let depth = 100_000;
    let src = format!(
        "fn main() -> i32 {{ return {}1{}; }}",
        "(".repeat(depth),
        ")".repeat(depth)
    );
    let e = rejected(&src);
    assert!(e.contains("nesting"), "{e}");
    let ok = format!(
        "fn main() -> i32 {{ return {}1{}; }}",
        "(".repeat(100),
        ")".repeat(100)
    );
    check(&ok, 1, "");
}

#[test]
fn partial_stdout_survives_a_runtime_error() {
    let src = "fn main() -> i32 { print_i32(1); print_i32(2); let z = 0; print_i32(5 / z); return 0; }";
    let opts = aether::CompileOptions {
        opt_level: 0,
        color: false,
    };
    let mut c = aether::compile_source("t.ae", src, &opts);
    assert!(!c.diags.has_errors());
    let err = aether::driver::run_compiled(&mut c).expect_err("division by zero");
    assert_eq!(err.stdout, "1\n2\n");
    assert!(err.to_string().contains("division by zero"));
}

#[test]
fn len_counts_chars_like_indexing() {
    check(
        "fn main() -> i32 { let s = \"ação\"; print_i32(len(s)); print_bool(s[2] == 'ã'); return len(s); }",
        4,
        "4\ntrue\n",
    );
}

// ---------- type system ----------

#[test]
fn negative_literals_and_expected_type_propagation() {
    let src = r#"
        fn main() -> i32 {
            let y: i64 = -1;
            let z: i64 = 0 - 1;
            let a: i64 = 5;
            let b = 1 + a;
            let c = a * 2 - 1;
            let d: i64 = -(2 + 3);
            let m = -2147483648;
            print_i64(y);
            print_i64(z);
            print_i64(b);
            print_i64(c);
            print_i64(d);
            print_i32(m);
            print_bool(1 < a);
            return 0;
        }
    "#;
    check(src, 0, "-1\n-1\n6\n9\n-5\n-2147483648\ntrue\n");
}

#[test]
fn function_names_are_not_values() {
    let e = rejected("fn g() -> i32 { return 5; }\nfn main() -> i32 { let f = g; return 0; }");
    assert!(e.contains("cannot be used as a value"), "{e}");
}

#[test]
fn for_bounds_must_be_i32() {
    let e = rejected("fn main() -> i32 { let n: i64 = 3; for i in 0..n { } return 0; }");
    assert!(e.contains("i32"), "{e}");
}

#[test]
fn tail_type_must_match_return_type() {
    let e = rejected("fn f() -> i32 { true }\nfn main() -> i32 { return f(); }");
    assert!(e.contains("tail expression"), "{e}");
}

// ---------- optimizer ----------

#[test]
fn leaf_functions_are_inlined() {
    let src = "fn add1(x: i32) -> i32 { return x + 1; }\nfn main() -> i32 { return add1(41); }";
    let opts = aether::CompileOptions {
        opt_level: 2,
        color: false,
    };
    let c = aether::compile_source("t.ae", src, &opts);
    let ir = aether::driver::dump_ir_text(&c, false);
    let main = ir.split("fn main").nth(1).expect("main in IR");
    assert!(!main.contains("call add1"), "inliner did not fire:\n{ir}");
    let report = c.opt_report.as_ref().unwrap();
    assert!(
        report.passes.iter().any(|p| p.name == "inline" && p.insts_after != p.insts_before),
        "{}",
        report.summary()
    );
    check(src, 42, "");
}

#[test]
fn expected_array_type_types_the_elements() {
    check(
        "fn g(a: [i64; 2]) -> i64 { return a[0] + a[1]; }\nfn main() -> i32 { let a: [i64; 2] = [1, 3000000000]; print_i64(g([3, 4])); print_i64(a[1]); let n: [[i64; 2]; 1] = [[5, 6]]; print_i64(n[0][1]); return 0; }",
        0,
        "7\n3000000000\n6\n",
    );
}

#[test]
fn negating_a_negative_literal_wraps_like_a_variable() {
    check(
        "fn main() -> i32 { let a = -(-2147483648); let b = -2147483648; let c = -b; print_i32(a); print_i32(c); let d: i64 = -(-2147483648); print_i64(d); return 0; }",
        0,
        "-2147483648\n-2147483648\n2147483648\n",
    );
}

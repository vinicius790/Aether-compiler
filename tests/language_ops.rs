//! Language expansion: compound assignment, bitwise/shift operators,
//! integer literal forms, `\u{...}` escapes, `len` on arrays, new built-ins.
//!
//! Programs whose new operators fold to constants (or that only use
//! operators the VM already executes) run at -O2 on the VM. Programs that
//! need the new backend opcodes with non-constant operands, or the new
//! natives, are compile-only checks until the backend wires them.

use aether::{compile_source, run_source, CompileOptions, Value};

fn run(src: &str, level: u8) -> (Value, String) {
    run_source("ops.ae", src, level)
        .map(|(v, out, _)| (v, out))
        .unwrap_or_else(|e| panic!("-O{level} failed:\n{e}\n{src}"))
}

/// Runs at -O2 and asserts the exit value and stdout.
fn check_o2(src: &str, value: i32, stdout: &str) {
    let (v, out) = run(src, 2);
    assert_eq!(v, Value::I32(value), "{src}");
    assert_eq!(out, stdout, "{src}");
}

/// Runs at -O0 and -O2 (only for programs the current VM executes).
fn check_both(src: &str, value: i32, stdout: &str) {
    let o0 = run(src, 0);
    let o2 = run(src, 2);
    assert_eq!(o0, o2, "-O0 and -O2 disagree\n{src}");
    assert_eq!(o0.0, Value::I32(value), "{src}");
    assert_eq!(o0.1, stdout, "{src}");
}

fn compiles(src: &str) {
    let opts = CompileOptions {
        opt_level: 2,
        color: false,
    };
    let c = compile_source("ops.ae", src, &opts);
    assert!(
        !c.diags.has_errors(),
        "{}\n{src}",
        c.diags.render(&c.session, false)
    );
    assert!(c.bytecode.is_some() && c.llvm.is_some(), "{src}");
}

fn rejected(src: &str) -> String {
    let opts = CompileOptions {
        opt_level: 0,
        color: false,
    };
    let c = compile_source("ops.ae", src, &opts);
    assert!(c.diags.has_errors(), "expected a compile error\n{src}");
    c.diags.render(&c.session, false)
}

// ---------- compound assignment (desugars to existing + - * / %) ----------

#[test]
fn compound_assignment_on_locals() {
    check_both(
        "fn main() -> i32 { let mut x = 5; x += 2; x *= 3; x -= 1; x /= 4; x %= 3; print_i32(x); return x; }",
        2,
        "2\n",
    );
    // the right operand is the whole expression: `x *= 2 + 3` is `x * 5`
    check_both(
        "fn main() -> i32 { let mut x = 2; x *= 2 + 3; return x; }",
        10,
        "",
    );
    // string concatenation
    check_both(
        r#"fn main() -> i32 { let mut s = "ab"; s += "cd"; print(s); return len(s); }"#,
        4,
        "abcd",
    );
}

#[test]
fn compound_assignment_on_array_element_and_struct_field() {
    check_both(
        r#"
        struct P { x: i32, y: i32 }
        fn main() -> i32 {
            let mut a = [1, 2, 3];
            a[1] += 40;
            a[0] *= 10;
            let mut p = P { x: 7, y: 1 };
            p.x -= 2;
            p.y += a[1];
            let mut g = [[1, 2], [3, 4]];
            g[1][0] += 100;
            print_i32(a[0]); print_i32(a[1]); print_i32(p.x); print_i32(p.y); print_i32(g[1][0]);
            return a[0] + a[1] + p.x + p.y + g[1][0];
        }
        "#,
        10 + 42 + 5 + 43 + 103,
        "10\n42\n5\n43\n103\n",
    );
    // the target is evaluated twice: a call in the index runs twice
    check_both(
        r#"
        fn idx() -> i32 { print("i"); return 0; }
        fn main() -> i32 { let mut a = [1]; a[idx()] += 1; return a[0]; }
        "#,
        2,
        "ii",
    );
}

#[test]
fn compound_assignment_respects_mutability_and_types() {
    let msg = rejected("fn main() -> i32 { let x = 1; x += 1; return x; }");
    assert!(msg.contains("E0241"), "{msg}");
    let msg = rejected("fn main() -> i32 { let mut x = 1; x += 1.5; return x; }");
    assert!(msg.contains("E0244"), "{msg}");
    let msg = rejected("fn main() -> i32 { let mut b = true; b <<= 1; return 0; }");
    assert!(msg.contains("E0244"), "{msg}");
}

// ---------- bitwise and shift operators ----------

#[test]
fn bitwise_constants_fold_and_run() {
    check_o2(
        r#"
        fn main() -> i32 {
            print_i32(0xFF & 0b1010);
            print_i32(0xF0 | 0x0F);
            print_i32(0xFF ^ 0x0F);
            print_i32(1 << 4);
            print_i32(256 >> 4);
            print_i32(-16 >> 2);
            print_i32(!0);
            print_i32(!0x0F & 0xFF);
            print_i32(1 << 32);
            print_i32(1 << 31);
            let m: i64 = !0;
            print_i64(m);
            print_i64(1 << 40);
            print_i64(0x7FFF_FFFF_FFFF_FFFF >> 60);
            print_bool(!true);
            return 0xFF & 0b1010;
        }
        "#,
        10,
        "10\n255\n240\n16\n16\n-4\n-1\n240\n1\n-2147483648\n-1\n1099511627776\n7\nfalse\n",
    );
}

#[test]
fn compound_bitwise_assignment_folds() {
    // sema + const-prop: the whole chain is constant, so the VM only sees loads
    check_o2(
        "fn main() -> i32 { let mut x = 0xF0; x &= 0x3C; x |= 1; x ^= 2; x <<= 1; x >>= 1; print_i32(x); return x; }",
        0x33,
        "51\n",
    );
}

#[test]
fn bitwise_with_opaque_operands_compiles() {
    // needs the new backend opcodes; compile-only until the VM executes them
    compiles("fn f(a: i32, b: i32) -> i32 { a & b }\nfn main() -> i32 { return f(6, 3); }");
    compiles("fn f(a: i32, b: i32) -> i32 { (a | b) ^ (a << b) ^ (a >> b) }\nfn main() -> i32 { return f(6, 3); }");
    compiles("fn f(a: i64, b: i64) -> i64 { (a & b) | (a ^ b) | (a << b) | (a >> b) }\nfn main() -> i32 { return f(6, 3) as i32; }");
    compiles("fn f(a: i32) -> i32 { !a }\nfn g(a: i64) -> i64 { !a }\nfn main() -> i32 { return f(1) + g(1) as i32; }");
    compiles("fn main() -> i32 { let mut x = 1; let mut i = 0; while i < 3 { x <<= 1; x |= i; i += 1; } return x; }");
}

#[test]
fn bitwise_type_rules() {
    let msg = rejected("fn main() -> i32 { let a: i64 = 1; let b: i32 = 1; return (a & b) as i32; }");
    assert!(msg.contains("E0244"), "{msg}");
    let msg = rejected("fn main() -> i32 { let a: i64 = 1; let b: i32 = 1; return (a << b) as i32; }");
    assert!(msg.contains("E0244"), "{msg}");
    let msg = rejected("fn main() -> i32 { return (1.0 & 2.0) as i32; }");
    assert!(msg.contains("E0244"), "{msg}");
    let msg = rejected("fn main() -> i32 { return true | false; }");
    assert!(msg.contains("E0244"), "{msg}");
    let msg = rejected("fn main() -> i32 { return !1.5 as i32; }");
    assert!(msg.contains("E0245"), "{msg}");
    // `&&`/`||` still require bool
    let msg = rejected("fn main() -> i32 { if 1 && 2 { return 1; } return 0; }");
    assert!(msg.contains("E0244"), "{msg}");
}

#[test]
fn precedence_binds_bitwise_tighter_than_comparison() {
    // `1 | 2 & 3 == 3` is `(1 | (2 & 3)) == 3` → true
    check_o2(
        "fn main() -> i32 { print_bool(1 | 2 & 3 == 3); print_bool(1 | 2 & 3 == 1); return 0; }",
        0,
        "true\nfalse\n",
    );
    // `1 << 2 + 3` is `1 << 5`; `6 & 3 + 1` is `6 & 4`
    check_o2(
        "fn main() -> i32 { print_i32(1 << 2 + 3); print_i32(6 & 3 + 1); print_i32(1 + 2 << 1); return 0; }",
        0,
        "32\n4\n6\n",
    );
}

// ---------- integer literal forms and escapes ----------

#[test]
fn integer_literal_forms() {
    check_both(
        r#"
        fn main() -> i32 {
            print_i32(0xFF);
            print_i32(0b1010);
            print_i32(0o17);
            print_i32(1_000_000);
            print_i64(0x7FFF_FFFF_FFFF_FFFF);
            let big: i64 = 0xFFFF_FFFF;
            print_i64(big);
            print_f64(1_000.5);
            return 0x2A;
        }
        "#,
        42,
        "255\n10\n15\n1000000\n9223372036854775807\n4294967295\n1000.5\n",
    );
    let msg = rejected("fn main() -> i32 { return 0xFFFF_FFFF_FFFF_FFFF; }");
    assert!(msg.contains("invalid integer literal"), "{msg}");
    let msg = rejected("fn main() -> i32 { return 0x; }");
    assert!(msg.contains("invalid integer literal"), "{msg}");
    // hex literals are values: 0xFFFF_FFFF does not fit i32
    let msg = rejected("fn main() -> i32 { return 0xFFFF_FFFF; }");
    assert!(msg.contains("E0263"), "{msg}");
}

#[test]
fn unicode_escapes() {
    // String escapes run on the VM. (`char` comparison and `len` of a
    // non-ASCII string are not exercised: the current VM has no char
    // compare opcode and counts bytes, see the report.)
    check_both(
        r#"
        fn main() -> i32 {
            let s = "\u{48}\u{49}\u{1F600}";
            print(s);
            let t = "\u{41}\u{42}\u{0043}";
            print(t);
            return len(t);
        }
        "#,
        3,
        "HI\u{1F600}ABC",
    );
    compiles(r"fn main() -> i32 { let c = '\u{41}'; let d = '\u{1F600}'; return 0; }");
    let msg = rejected(r#"fn main() -> i32 { let s = "\u{110000}"; return 0; }"#);
    assert!(msg.contains("E0005"), "{msg}");
    let msg = rejected(r"fn main() -> i32 { let c = '\u{}'; return 0; }");
    assert!(msg.contains("E0005"), "{msg}");
}

// ---------- len on arrays, built-ins ----------

#[test]
fn len_accepts_strings_and_arrays() {
    check_both(
        r#"
        fn main() -> i32 {
            let a = [1, 2, 3];
            let g = [[1, 2], [3, 4], [5, 6], [7, 8]];
            print_i32(len(a));
            print_i32(len(g));
            print_i32(len(g[0]));
            print_i32(len("hello"));
            return len([1, 2, 3]);
        }
        "#,
        3,
        "3\n4\n2\n5\n",
    );
    let msg = rejected("fn main() -> i32 { return len(3); }");
    assert!(msg.contains("E0258"), "{msg}");
}

#[test]
fn new_builtins_compile() {
    // natives 8..=20 need VM support; compile-only here
    compiles(
        r#"
        fn main() -> i32 {
            print_char('a');
            print(to_string(42));
            print(i64_to_string(42 as i64));
            print(f64_to_string(1.5));
            print(char_to_string('x'));
            print_i32(abs(0 - 5) + min(1, 2) + max(1, 2) + clamp(15, 0, 10) + pow_i32(2, 10));
            print_f64(sqrt(2.0) + floor(1.5) + ceil(1.5));
            return 0;
        }
        "#,
    );
    let msg = rejected("fn main() -> i32 { return abs(1.5); }");
    assert!(msg.contains("E0258"), "{msg}");
    let msg = rejected("fn main() -> i32 { return min(1); }");
    assert!(msg.contains("E0257"), "{msg}");
}

#[test]
fn user_functions_shadow_builtins() {
    // stdlib/math.ae defines abs/min/max/clamp and must keep running
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/stdlib/math.ae")).unwrap();
    check_both(&src, 0, "5\n3\n9\n10\n");
    // a user `fn` wins even with a different signature
    check_both(
        r#"
        fn abs(x: f64) -> f64 { if x < 0.0 { return 0.0 - x; } return x; }
        fn min(a: i32, b: i32, c: i32) -> i32 { if a < b { if a < c { return a; } return c; } if b < c { return b; } return c; }
        fn main() -> i32 { print_f64(abs(0.0 - 2.5)); return min(3, 1, 2); }
        "#,
        1,
        "2.5\n",
    );
}

// ---------- robustness ----------

#[test]
fn deeply_nested_parens_type_check_on_default_stack() {
    // `check_expr` is a thin dispatcher now, so sema no longer needs a big
    // stack: the whole front end runs here on the default 2 MiB test thread.
    let n = aether::parser::MAX_NESTING - 8;
    let src = format!(
        "fn main() -> i32 {{ let x = {}1{}; return x; }}",
        "(".repeat(n),
        ")".repeat(n)
    );
    let opts = CompileOptions {
        opt_level: 2,
        color: false,
    };
    let c = compile_source("deep.ae", &src, &opts);
    assert!(!c.diags.has_errors(), "{}", c.diags.render(&c.session, false));
    let (v, _) = run(&src, 2);
    assert_eq!(v, Value::I32(1));
}

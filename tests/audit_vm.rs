//! Adversarial audit of the execution core: copy-on-write values, integer and
//! float semantics, strings, step/depth limits, resumable runs, host
//! bindings, optimizer soundness (pass by pass, on hand-built IR where
//! source programs cannot reach the case), register allocation and the
//! assembler's refusal of malformed IR.
//!
//! Source programs run at -O0 and -O2; both must give the same result and
//! the value derived by hand from `docs/language.md`.

use aether::ast::BinOp;
use aether::backend::bytecode::{BcFunction, BytecodeModule, Immediate, Op, MAX_FIELDS};
use aether::backend::assemble;
use aether::host::Host;
use aether::ir::{BasicBlock, BlockId, ConstValue, Inst, IrFunction, IrModule, Reg, Terminator};
use aether::opt::{self, optimize};
use aether::span::Span;
use aether::ty::Type;
use aether::vm::{Step, Value, Vm, VmError, VmOptions};
use aether::{compile_file, compile_source, run_source, CompileOptions};
use std::cell::RefCell;
use std::rc::Rc;

// ---------------------------------------------------------------- helpers

fn run(src: &str, level: u8) -> Result<(Value, String), String> {
    run_source("audit.ae", src, level).map(|(v, out, _)| (v, out))
}

/// Runs at -O0 and -O2 and asserts both agree; returns the common outcome.
fn both(src: &str) -> Result<(Value, String), String> {
    let o0 = run(src, 0);
    let o2 = run(src, 2);
    assert_eq!(o0, o2, "-O0 and -O2 disagree\n{src}");
    o0
}

/// Both levels succeed with exactly this stdout.
fn out(src: &str, want: &str) {
    match both(src) {
        Ok((_, got)) => assert_eq!(got, want, "{src}"),
        Err(e) => panic!("unexpected error: {e}\n{src}"),
    }
}

/// Both levels fail at runtime with a message containing `needle`.
fn runtime_error(src: &str, needle: &str) {
    match both(src) {
        Err(e) => assert!(e.contains(needle), "error `{e}` lacks `{needle}`\n{src}"),
        Ok(v) => panic!("expected a runtime error containing `{needle}`, got {v:?}\n{src}"),
    }
}

fn module_of(functions: Vec<IrFunction>) -> IrModule {
    IrModule {
        functions,
        structs: Vec::new(),
    }
}

fn func(name: &str, params: Vec<Type>, reg_count: u32, blocks: Vec<BasicBlock>) -> IrFunction {
    IrFunction {
        name: name.to_string(),
        params: params
            .into_iter()
            .enumerate()
            .map(|(i, t)| (format!("p{i}"), t, Reg(i as u32)))
            .collect(),
        return_ty: Type::I32,
        blocks,
        reg_count,
        is_extern: false,
        span: Span::DUMMY,
    }
}

fn block(id: u32, insts: Vec<Inst>, term: Terminator) -> BasicBlock {
    BasicBlock {
        id: BlockId(id),
        insts,
        term,
    }
}

fn konst(dest: u32, v: i32) -> Inst {
    Inst::LoadConst {
        dest: Reg(dest),
        value: ConstValue::I32(v),
    }
}

fn bin(dest: u32, op: BinOp, ty: Type, lhs: u32, rhs: u32) -> Inst {
    Inst::Bin {
        dest: Reg(dest),
        op,
        ty,
        lhs: Reg(lhs),
        rhs: Reg(rhs),
    }
}

fn ret(r: u32) -> Terminator {
    Terminator::Return { value: Some(Reg(r)) }
}

fn jump(b: u32) -> Terminator {
    Terminator::Jump { target: BlockId(b) }
}

fn run_ir(m: &IrModule) -> Result<Value, String> {
    let bc = assemble(m)?;
    aether::vm::execute_captured(&bc).map(|(v, _, _)| v).map_err(|e| e.to_string())
}

type Pass = fn(&mut IrModule);

/// Every pass in isolation plus the pipelines, each must preserve the
/// result of `m` (value or error text).
fn assert_all_passes_preserve(m: &IrModule) {
    let want = run_ir(m);
    let passes: &[(&str, Pass)] = &[
        ("const-fold", opt::pass_const_fold),
        ("const-prop", opt::pass_const_prop),
        ("algebraic", opt::pass_algebraic),
        ("local-cse", opt::pass_local_cse),
        ("copy-prop", opt::pass_copy_prop),
        ("cf-simplify", opt::pass_cf_simplify),
        ("dce", opt::pass_dce),
        ("inline", opt::inline::pass_inline),
        ("dead-fn", opt::pass_dead_functions),
        ("regalloc", opt::regalloc::pass_regalloc),
    ];
    for (name, pass) in passes {
        let mut c = m.clone();
        pass(&mut c);
        assert_eq!(run_ir(&c), want, "pass `{name}` changed the result\n{}", aether::ir::dump_ir(&c));
    }
    for level in [1u8, 2] {
        let (c, _) = optimize(m.clone(), level);
        assert_eq!(run_ir(&c), want, "-O{level} changed the result\n{}", aether::ir::dump_ir(&c));
    }
}

// ------------------------------------------------- integer / float values

#[test]
fn integer_edges_wrap_and_shifts_mask() {
    out(
        r#"
        fn main() -> i32 {
            let m = 0 - 2147483647 - 1;
            let n1 = 0 - 1;
            print_i32(m / n1); print_i32(m % n1); print_i32(0 - m); print_i32(abs(m));
            print_i32(m * n1); print_i32(m - 1); print_i32(m + m);
            let a: i64 = 0 - 9223372036854775807 - 1;
            let b: i64 = 0 - 1;
            print_i64(a / b); print_i64(a % b); print_i64(-a); print_i64(a - 1); print_i64(a * b);
            print_i32(1 << 32); print_i32(1 << 33); print_i32(1 << (0-1)); print_i32((0-8) >> 1); print_i32((0-8) >> 33);
            let s: i64 = 1; print_i64(s << 64); print_i64(s << 65); print_i64(s << 63); print_i64((s << 63) >> 63);
            print_i32(!0); print_i64(!(0 as i64));
            return 0;
        }"#,
        "-2147483648\n0\n-2147483648\n-2147483648\n-2147483648\n2147483647\n0\n\
         -9223372036854775808\n0\n-9223372036854775808\n9223372036854775807\n-9223372036854775808\n\
         1\n2\n-2147483648\n-4\n-4\n1\n2\n-9223372036854775808\n-1\n-1\n-1\n",
    );
}

#[test]
fn division_signs_follow_truncation() {
    out(
        r#"fn main() -> i32 {
            print_i32(7 / (0-2)); print_i32(7 % (0-2)); print_i32((0-7) / 2); print_i32((0-7) % 2);
            print_i64(7 / (0-2 as i64)); print_i64((0-7 as i64) % 2);
            return 0;
        }"#,
        "-3\n1\n-3\n-1\n-3\n-1\n",
    );
}

#[test]
fn float_edges_and_printing() {
    out(
        r#"
        fn main() -> i32 {
            let nan = 0.0 / 0.0;
            let inf = 1.0 / 0.0;
            let ninf = -1.0 / 0.0;
            print_f64(nan); print_f64(inf); print_f64(ninf); print_f64(-0.0); print_f64(0.0 - 0.0);
            print_f64(1e21); print_f64(1e-7); print_f64(123456789012345680000.0);
            print_f64(0.1 + 0.2); print_f64(1e15); print_f64(1e16);
            print_bool(nan == nan); print_bool(nan != nan); print_bool(nan < 1.0); print_bool(nan >= 1.0);
            print_bool(nan > nan); print_bool(nan <= nan);
            print_bool(0.0 == -0.0); print_bool(-0.0 < 0.0); print_bool(-0.0 >= 0.0);
            return 0;
        }"#,
        "NaN\ninf\n-inf\n-0\n0\n1000000000000000000000\n0.0000001\n123456789012345680000\n\
         0.30000000000000004\n1000000000000000\n10000000000000000\n\
         false\ntrue\nfalse\nfalse\nfalse\nfalse\ntrue\nfalse\ntrue\n",
    );
}

#[test]
fn float_casts_saturate_and_nan_is_zero() {
    out(
        r#"
        fn main() -> i32 {
            let nan = 0.0 / 0.0; let inf = 1.0 / 0.0; let ninf = -1.0 / 0.0;
            print_i32(nan as i32); print_i32(inf as i32); print_i32(ninf as i32);
            print_i32(3000000000.0 as i32); print_i32(-3000000000.0 as i32);
            print_i32(2.9 as i32); print_i32(-2.9 as i32);
            print_i64(nan as i64); print_i64(inf as i64); print_i64(ninf as i64);
            print_i64(1e30 as i64); print_i64(-1e30 as i64); print_i64(9.3e18 as i64);
            return 0;
        }"#,
        "0\n2147483647\n-2147483648\n2147483647\n-2147483648\n2\n-2\n\
         0\n9223372036854775807\n-9223372036854775808\n9223372036854775807\n-9223372036854775808\n9223372036854775807\n",
    );
}

#[test]
fn integer_and_char_casts() {
    out(
        r#"
        fn main() -> i32 {
            let big: i64 = 4294967296;
            print_i32(big as i32); print_i32((big + 5) as i32); print_i32((big - 1) as i32);
            let neg: i64 = 0 - 2147483649; print_i32(neg as i32);
            let x: i32 = 0 - 1; print_i64(x as i64);
            print_i32(55296 as char as i32); print_i32(1114112 as char as i32);
            print_i32((0-1) as char as i32); print_i32(65 as char as i32); print_i32(0 as char as i32);
            return 0;
        }"#,
        "0\n5\n-1\n2147483647\n-1\n65533\n65533\n65533\n65\n0\n",
    );
}

#[test]
fn builtin_edge_cases() {
    out(
        r#"
        fn main() -> i32 {
            print_i32(pow_i32(2, 31)); print_i32(pow_i32(2, 32)); print_i32(pow_i32(0-2, 31));
            print_i32(pow_i32(3, 0-1)); print_i32(pow_i32(0, 0));
            print_i32(min(1, 0-1)); print_i32(clamp(5, 10, 0)); print_i32(clamp(5, 0, 10)); print_i32(clamp(0-5, 0, 10));
            print_f64(sqrt(0.0 - 1.0)); print_f64(floor(0.0 - 0.5)); print_f64(ceil(0.0 - 0.5)); print_f64(floor(1.0/0.0));
            return 0;
        }"#,
        "-2147483648\n0\n-2147483648\n0\n1\n-1\n10\n5\n0\nNaN\n-1\n-0\ninf\n",
    );
}

#[test]
fn constant_folding_matches_runtime_for_floats_and_bools() {
    // Same expressions with constant operands (folded) and with operands the
    // optimizer cannot see through (extern-free: loop-carried values).
    out(
        r#"
        fn opq() -> f64 { return -0.0; }
        fn main() -> i32 {
            let x = opq();
            print_f64(x * 1.0); print_f64(x + 0.0); print_f64(x - 0.0); print_f64(x * 0.0);
            print_f64(x - x); print_bool(x == x); print_f64(x / 1.0);
            let n = 0.0 / 0.0;
            print_f64(n * 0.0); print_f64(n - n); print_f64(1.0 * n); print_f64(n + 0.0);
            print_f64(-0.0 + 0.0); print_f64(-0.0 * 1.0); print_f64(-(0.0)); print_f64(-0.0 - 0.0);
            print_bool(true == false); print_bool(true != false); print_bool(!true);
            print_bool('b' <= 'a'); print_bool('a' < 'b');
            return 0;
        }"#,
        "-0\n0\n-0\n-0\n0\ntrue\n-0\nNaN\nNaN\nNaN\nNaN\n0\n-0\n-0\n-0\nfalse\ntrue\nfalse\nfalse\ntrue\n",
    );
}

// ----------------------------------------------------------------- strings

#[test]
fn string_length_and_indexing_count_chars() {
    out(
        r#"
        fn main() -> i32 {
            let s = "ação→😀";
            print_i32(len(s));
            print_char(s[0]); print_char(s[1]); print_char(s[2]); print_char(s[3]); print_char(s[4]); print_char(s[5]);
            print_i32(s[5] as i32);
            print_i32(len("e\u{301}"));
            print_bool("é" == "e\u{301}");
            print_bool("a" + "b" == "ab"); print_bool("" + "" == "");
            return 0;
        }"#,
        "6\na\nç\nã\no\n→\n😀\n128512\n2\nfalse\ntrue\ntrue\n",
    );
}

#[test]
fn string_index_out_of_range_is_a_clean_error() {
    runtime_error(r#"fn main() -> i32 { let s = "ab"; print_char(s[2]); return 0; }"#, "string index out of bounds");
    runtime_error(
        r#"fn main() -> i32 { let s = "ab"; let i = 0 - 1; print_char(s[i]); return 0; }"#,
        "string index out of bounds",
    );
    runtime_error(r#"fn main() -> i32 { let s = ""; print_char(s[0]); return 0; }"#, "string index out of bounds");
}

#[test]
fn doubling_a_string_hits_the_length_limit_instead_of_aborting() {
    runtime_error(
        r#"fn main() -> i32 {
            let mut s = "xxxxxxxx";
            let mut i = 0;
            while i < 60 { s = s + s; i = i + 1; }
            return len(s);
        }"#,
        "exceeds the limit",
    );
}

#[test]
fn string_constant_folding_is_bounded() {
    // 2^17 bytes by constant doubling: folding stops, the VM computes it.
    let mut src = String::from("fn main() -> i32 {\n let s0 = \"x\";\n");
    for i in 1..=17 {
        src.push_str(&format!(" let s{i} = s{} + s{};\n", i - 1, i - 1));
    }
    src.push_str(" return len(s17);\n}\n");
    let (v, _) = both(&src).unwrap();
    assert_eq!(v, Value::I32(1 << 17));
}

// ----------------------------------------------------------- copy-on-write

#[test]
fn cow_nested_aggregates_never_alias() {
    out(
        r#"
        struct In { a: [i32; 2] }
        struct Out { i: In, k: i32 }
        fn mutate(o0: Out) -> i32 { let mut o = o0; o.i.a[0] = 100; o.k = 5; return o.i.a[0] + o.k; }
        fn mutate2(a0: [Out; 2]) -> [Out; 2] { let mut a = a0; a[1].i.a[1] = 77; return a; }
        fn main() -> i32 {
            let mut o = Out { i: In { a: [1, 2] }, k: 3 };
            let o2 = o;
            let r = mutate(o);
            print_i32(r); print_i32(o.i.a[0]); print_i32(o.k);
            o.i.a[1] = 9;
            print_i32(o2.i.a[1]); print_i32(o.i.a[1]);
            let arr = [o, o2];
            let arr2 = mutate2(arr);
            print_i32(arr[1].i.a[1]); print_i32(arr2[1].i.a[1]); print_i32(arr2[0].i.a[1]);
            let mut m = [[1, 2], [3, 4]];
            let row = m[0];
            m[0][0] = 50;
            print_i32(row[0]); print_i32(m[0][0]);
            let mut grid = [[0, 0, 0], [0, 0, 0], [0, 0, 0]];
            let mut i = 0;
            while i < 3 { grid[i][i] = i + 1; i = i + 1; }
            print_i32(grid[0][0] + grid[1][1] + grid[2][2] + grid[0][1]);
            return 0;
        }"#,
        "105\n1\n3\n2\n9\n2\n77\n9\n1\n50\n6\n",
    );
}

#[test]
fn cow_tuples_and_destructuring() {
    out(
        r#"
        fn main() -> i32 {
            let mut a = [1, 2, 3];
            let b = a;
            a[0] = 10;
            let mut c = b;
            c[1] = 20;
            let t = (a, b, c);
            let mut u = t;
            u.0[2] = 30;
            print_i32(a[0]+a[1]+a[2]); print_i32(b[0]+b[1]+b[2]); print_i32(c[0]+c[1]+c[2]);
            print_i32(t.0[2]); print_i32(u.0[2]);
            let (x, y0, z) = u;
            let mut y = y0;
            y[0] = 99;
            print_i32(x[0]); print_i32(y[0]); print_i32(u.1[0]); print_i32(z[1]);
            return 0;
        }"#,
        "15\n6\n24\n3\n30\n10\n99\n1\n20\n",
    );
}

#[test]
fn cow_enum_payload_arrays_and_aggregate_equality() {
    out(
        r#"
        enum E { A([i32; 2]), B(i32, i32), C }
        fn f(e: E) -> i32 {
            match e {
                E::A(v) => { let mut w = v; w[0] = 5; return w[0] + v[0]; }
                E::B(x, y) => { return x + y; }
                _ => { return 0; }
            }
        }
        fn main() -> i32 {
            let a = [1, 2];
            let e = E::A(a);
            print_i32(f(e)); print_i32(f(e));
            let e2 = E::B(3, 4);
            print_i32(f(e2));
            if let E::A(q0) = e { let mut q = q0; q[1] = 77; print_i32(q[1]); }
            if let E::A(q) = e { print_i32(q[1]); }
            print_bool(e == E::A([1, 2])); print_bool(e == E::A([1, 3]));
            print_bool(e2 == E::B(3, 4)); print_bool(E::C == E::C); print_bool(e == E::C);
            return 0;
        }"#,
        "6\n6\n7\n77\n2\ntrue\nfalse\ntrue\ntrue\nfalse\n",
    );
}

#[test]
fn array_equality_is_elementwise_and_survives_mutation() {
    // `==` on arrays is accepted by sema; the backend used to refuse it
    // (E0300), and local CSE must not reuse a comparison across a store
    // into one of the compared arrays.
    out(
        r#"
        fn main() -> i32 {
            let mut a = [1, 2, 3];
            let b = [1, 2, 3];
            let e1 = a == b;
            a[1] = 9;
            let e2 = a == b;
            let e3 = a != b;
            let m = [[1, 2], [3, 4]];
            let n = [[1, 2], [3, 4]];
            print_bool(e1); print_bool(e2); print_bool(e3); print_bool(m == n);
            let f = [0.0 / 0.0];
            print_bool(f == f);
            return 0;
        }"#,
        "true\nfalse\ntrue\ntrue\nfalse\n",
    );
}

#[test]
fn cow_recursion_with_values_passed_down() {
    out(
        r#"
        fn rec(a0: [i32; 3], n: i32) -> i32 {
            let mut a = a0;
            if n == 0 { return a[0] + a[1] + a[2]; }
            a[n % 3] = a[n % 3] + n;
            let r = rec(a, n - 1);
            return r * 2 + a[0];
        }
        fn main() -> i32 {
            let a = [1, 1, 1];
            print_i32(rec(a, 6));
            print_i32(a[0] + a[1] + a[2]);
            return 0;
        }"#,
        "2145\n3\n",
    );
}

#[test]
fn cow_array_of_structs_updated_in_loops() {
    out(
        r#"
        struct P { x: i32, y: i32 }
        fn bump(p0: P) -> P { let mut p = p0; p.x = p.x + 1; return p; }
        fn main() -> i32 {
            let mut ps = [P { x: 1, y: 2 }, P { x: 3, y: 4 }];
            let q = ps[0];
            ps[0] = bump(ps[0]);
            ps[1] = bump(bump(ps[0]));
            print_i32(q.x); print_i32(ps[0].x); print_i32(ps[1].x);
            let mut i = 0;
            while i < 2 { ps[i].y = ps[i].y * 10; ps[i] = bump(ps[i]); i = i + 1; }
            print_i32(ps[0].x); print_i32(ps[0].y); print_i32(ps[1].x); print_i32(ps[1].y);
            return 0;
        }"#,
        "1\n2\n4\n3\n20\n5\n20\n",
    );
}

#[test]
fn cow_large_literal_array_copy_is_independent() {
    let n = 5000;
    let lit = vec!["7"; n].join(", ");
    let src = format!(
        "fn main() -> i32 {{ let big = [{lit}]; let mut b2 = big; b2[{}] = 1; \
         print_i32(big[{}]); print_i32(b2[{}]); print_i32(len(big)); return 0; }}",
        n - 1,
        n - 1,
        n - 1
    );
    out(&src, &format!("7\n1\n{n}\n"));
}

// ------------------------------------------- uninitialised `let` (see vm.md)

#[test]
fn uninitialised_scalars_default_to_zero() {
    out(
        r#"fn main() -> i32 {
            let x: i32; let y: i64; let f: f64; let b: bool; let s: string; let c: char;
            print_i32(x); print_i64(y); print_f64(f); print_bool(b); print(s); println("|");
            print_i32(c as i32);
            let mut z: i32; z = z + 1; print_i32(z);
            return x;
        }"#,
        "0\n0\n0\nfalse\n|\n0\n1\n",
    );
}

#[test]
fn uninitialised_aggregates_never_yield_silent_garbage() {
    // Whatever the frontend decides for `let a: [i32; 3];` (default value or
    // rejection), the VM must not let the program read `()` as a number or
    // drop a store: the run either fails cleanly or computes the right thing.
    for (src, right) in [
        ("fn main() -> i32 { let mut a: [i32; 3]; a[1] = 5; print_i32(a[0] + a[1]); return 0; }", "5\n"),
        ("struct S { a: i32, b: i32 } fn main() -> i32 { let mut s: S; s.a = 1; print_i32(s.a); return 0; }", "1\n"),
        ("fn main() -> i32 { let t: (i32, bool); print_i32(t.0); return 0; }", "0\n"),
        ("fn main() -> i32 { let a: [i32; 3]; print_i32(len(a)); return 0; }", "3\n"),
    ] {
        match both(src) {
            Ok((_, got)) => assert_eq!(got, right, "{src}"),
            Err(e) => assert!(e.contains("unit"), "unexpected error `{e}` for {src}"),
        }
    }
}

// --------------------------------------------- optimizer keeps runtime errors

#[test]
fn dead_trapping_instructions_survive_dce() {
    for (src, msg) in [
        ("fn main() -> i32 { let z = 0; let x = 5 / z; print_i32(1); return 0; }", "division by zero"),
        ("fn main() -> i32 { let z = 0; let x = 5 % z; print_i32(1); return 0; }", "division by zero"),
        ("fn main() -> i32 { let z: i64 = 0; let x = 5 / z; print_i32(1); return 0; }", "division by zero"),
        ("fn f(a: i32, b: i32) -> i32 { return a / b; } fn main() -> i32 { let x = f(1, 0); print_i32(1); return 0; }", "division by zero"),
        ("fn main() -> i32 { let a = [1,2,3]; let i = 5; let x = a[i]; print_i32(1); return 0; }", "out of bounds"),
        (r#"fn main() -> i32 { let s = "abc"; let i = 5; let x = s[i]; print_i32(1); return 0; }"#, "out of bounds"),
    ] {
        runtime_error(src, msg);
    }
    // a divisor known to be nonzero does not keep a dead division alive
    let ir = {
        let c = compile_source(
            "d.ae",
            "fn main() -> i32 { let z = 4; let x = 100 / z; let y = 7 % 3; return 0; }",
            &CompileOptions { opt_level: 0, color: false },
        );
        c.ir_unopt.unwrap()
    };
    let (opt, _) = optimize(ir, 2);
    let text = aether::ir::dump_ir(&opt);
    assert!(!text.contains("/.i32") && !text.contains("%.i32"), "{text}");
}

#[test]
fn trapping_division_is_kept_by_every_level_even_when_unused() {
    for level in [0u8, 1, 2] {
        let e = run("fn main() -> i32 { let z = 0; let q = 1 / z; return 3; }", level).unwrap_err();
        assert!(e.contains("division by zero"), "-O{level}: {e}");
    }
}

// ------------------------------------------------------ struct / tuple size

fn wide_struct(n: usize) -> String {
    let fields: Vec<String> = (0..n).map(|i| format!("q{i}: i32")).collect();
    let init: Vec<String> = (0..n).map(|i| format!("q{i}: {i}")).collect();
    format!(
        "struct S {{ {} }}\nfn main() -> i32 {{ let mut s = S {{ {} }}; s.q{} = 7; \
         print_i32(s.q{}); print_i32(s.q0); print_i32(s.q{}); return 0; }}\n",
        fields.join(", "),
        init.join(", "),
        n - 1,
        n - 1,
        n / 2
    )
}

#[test]
fn objects_wider_than_a_byte_index_correctly() {
    for n in [255usize, 256, 257, 300] {
        out(&wide_struct(n), &format!("7\n0\n{}\n", n / 2));
    }
    let n = 300;
    let ty = vec!["i32"; n].join(", ");
    let vals: Vec<String> = (0..n).map(|i| i.to_string()).collect();
    out(
        &format!(
            "fn main() -> i32 {{ let t: ({ty}) = ({}); print_i32(t.299); print_i32(t.1); print_i32(t.256); return 0; }}",
            vals.join(", ")
        ),
        "299\n1\n256\n",
    );
    let payload = vec!["i32"; n].join(", ");
    let pats: Vec<String> = (0..n).map(|i| format!("p{i}")).collect();
    out(
        &format!(
            "enum E {{ A({payload}), B }}\nfn main() -> i32 {{ let e = E::A({}); match e {{ E::A({}) => {{ print_i32(p299); print_i32(p1); print_i32(p256); return 0; }} _ => {{ return 1; }} }} }}",
            vals.join(", "),
            pats.join(", ")
        ),
        "299\n1\n256\n",
    );
}

// ------------------------------------------------------- optimizer, source

#[test]
fn inliner_handles_param_reassignment_unit_leaves_and_nesting() {
    out(
        r#"
        fn sw(a: i32, b: i32) -> i32 { let mut x = a; let mut y = b; let t = x; x = y; y = t; return x * 10 + y; }
        fn unit_leaf(n: i32) { let q = n + 1; }
        fn pr(n: i32) { print_i32(n); }
        fn ret_param(a: i32) -> i32 { return a; }
        fn main() -> i32 {
            let a = 1; let b = 2;
            print_i32(sw(a, b)); print_i32(sw(b, a)); print_i32(sw(sw(a, b), sw(b, a)));
            unit_leaf(3);
            pr(sw(4, 5));
            let mut i = 0;
            let mut s = 0;
            while i < 5 { s = s + ret_param(i) * sw(i, s); i = i + 1; pr(s); }
            print_i32(a); print_i32(b);
            return s;
        }"#,
        "21\n12\n141\n54\n0\n1\n25\n784\n32160\n1\n2\n",
    );
}

#[test]
fn mutual_recursion_and_loops_are_never_inlined() {
    let src = r#"
        fn even(n: i32) -> bool { if n == 0 { return true; } return odd(n - 1); }
        fn odd(n: i32) -> bool { if n == 0 { return false; } return even(n - 1); }
        fn looped(n: i32) -> i32 { let mut s = 0; let mut i = 0; while i < n { s = s + i; i = i + 1; } return s; }
        fn leaf(n: i32) -> i32 { return n + 1; }
        fn caller(n: i32) -> i32 { return leaf(n) + leaf(n + 1); }
        fn main() -> i32 { print_bool(even(10)); print_bool(odd(7)); print_i32(caller(1)); print_i32(looped(5)); return 0; }
    "#;
    out(src, "true\ntrue\n5\n10\n");
    let c = compile_source("m.ae", src, &CompileOptions { opt_level: 2, color: false });
    let ir = c.ir.unwrap();
    assert!(ir.function("even").is_some() && ir.function("odd").is_some() && ir.function("looped").is_some());
    assert!(ir.function("leaf").is_none(), "leaf should be inlined away");
}

#[test]
fn thousands_of_inlined_call_sites_compile_quickly_and_correctly() {
    let n = 1000;
    let mut src = String::from(
        "fn leaf(a: i32, b: i32) -> i32 { let c = a * 3 + b; let d = c - a; let e = d * d; return e % 1000 + c; }\n\
         fn main() -> i32 {\n let mut s = 0;\n",
    );
    for i in 0..n {
        src.push_str(&format!(" s = s + leaf({i}, s);\n"));
    }
    src.push_str(" print_i32(s);\n return 0;\n}\n");
    let t = std::time::Instant::now();
    let (_, o) = both(&src).unwrap();
    assert!(!o.is_empty());
    assert!(t.elapsed().as_secs() < 20, "quadratic optimizer? {:?}", t.elapsed());
}

#[test]
fn many_independent_branches_optimize_in_reasonable_time() {
    let n = 1000;
    let mut src = String::from("fn main() -> i32 {\n let mut s = 0;\n");
    for i in 0..n {
        src.push_str(&format!(
            " let a{i} = s + {i};\n if a{i} % 3 == 0 {{ s = s + a{i}; }} else {{ s = s - 1; }}\n"
        ));
    }
    src.push_str(" print_i32(s);\n return 0;\n}\n");
    let t = std::time::Instant::now();
    both(&src).unwrap();
    assert!(t.elapsed().as_secs() < 30, "{:?}", t.elapsed());
}

// --------------------------------------------------- optimizer, hand-built IR

#[test]
fn cse_does_not_reuse_an_entry_whose_operand_was_overwritten() {
    // %0 = 5; %1 = 1; %0 = %0 + %1 (6); %3 = %0 + %1 must be 7, not a copy of %0.
    let m = module_of(vec![func(
        "main",
        vec![],
        4,
        vec![block(
            0,
            vec![
                konst(0, 5),
                konst(1, 1),
                bin(0, BinOp::Add, Type::I32, 0, 1),
                bin(3, BinOp::Add, Type::I32, 0, 1),
            ],
            ret(3),
        )],
    )]);
    assert_eq!(run_ir(&m), Ok(Value::I32(7)));
    assert_all_passes_preserve(&m);
}

#[test]
fn cse_keeps_distinct_results_when_the_result_register_is_reused() {
    // %2 = %0 + %1; %2 = 100 (result overwritten); %3 = %0 + %1 must recompute.
    let m = module_of(vec![func(
        "main",
        vec![],
        5,
        vec![block(
            0,
            vec![
                konst(0, 5),
                konst(1, 1),
                bin(2, BinOp::Add, Type::I32, 0, 1),
                konst(2, 100),
                bin(3, BinOp::Add, Type::I32, 0, 1),
                bin(4, BinOp::Add, Type::I32, 2, 3),
            ],
            ret(4),
        )],
    )]);
    assert_eq!(run_ir(&m), Ok(Value::I32(106)));
    assert_all_passes_preserve(&m);
}

fn array_eq_across_store(store_between: bool) -> IrModule {
    // a = [1]; b = [1]; e1 = a == b; (a[0] = 2;) e2 = a == b; ret e1*10 + e2
    let arr = Type::Array {
        elem: Box::new(Type::I32),
        len: 1,
    };
    let mut insts = vec![
        Inst::AllocArray { dest: Reg(0), elem: Type::I32, len: 1 },
        Inst::AllocArray { dest: Reg(1), elem: Type::I32, len: 1 },
        konst(2, 0),
        konst(3, 1),
        Inst::IndexStore { base: Reg(0), index: Reg(2), value: Reg(3), elem: Type::I32 },
        Inst::IndexStore { base: Reg(1), index: Reg(2), value: Reg(3), elem: Type::I32 },
        bin(4, BinOp::Eq, arr.clone(), 0, 1),
    ];
    if store_between {
        insts.push(konst(5, 2));
        insts.push(Inst::IndexStore { base: Reg(0), index: Reg(2), value: Reg(5), elem: Type::I32 });
    }
    insts.extend([
        bin(6, BinOp::Eq, arr, 0, 1),
        Inst::Cast { dest: Reg(7), src: Reg(4), from: Type::Bool, to: Type::I32 },
        Inst::Cast { dest: Reg(8), src: Reg(6), from: Type::Bool, to: Type::I32 },
        konst(9, 10),
        bin(10, BinOp::Mul, Type::I32, 7, 9),
        bin(11, BinOp::Add, Type::I32, 10, 8),
    ]);
    module_of(vec![func("main", vec![], 12, vec![block(0, insts, ret(11))])])
}

#[test]
fn cse_does_not_cross_a_store_into_a_compared_array() {
    let m = array_eq_across_store(true);
    assert_eq!(run_ir(&m), Ok(Value::I32(10)));
    assert_all_passes_preserve(&m);
    let m = array_eq_across_store(false);
    assert_eq!(run_ir(&m), Ok(Value::I32(11)));
    assert_all_passes_preserve(&m);
}

#[test]
fn liveness_reaches_a_fixpoint_on_a_backwards_laid_out_chain() {
    // bb0: %0 = 7; jmp bb1 ... bbN: ret %0, laid out as bb0, bbN, ..., bb1 so
    // every liveness sweep moves the fact one block. A capped sweep count
    // used to let DCE delete the definition of %0.
    let n = 300u32;
    let mut blocks = vec![block(0, vec![konst(0, 7)], jump(1))];
    for k in (1..=n).rev() {
        if k == n {
            blocks.push(block(k, vec![Inst::Nop], ret(0)));
        } else {
            blocks.push(block(k, vec![Inst::Nop], jump(k + 1)));
        }
    }
    let m = module_of(vec![func("main", vec![], 1, blocks)]);
    assert_eq!(run_ir(&m), Ok(Value::I32(7)));
    let mut d = m.clone();
    opt::pass_dce(&mut d);
    assert_eq!(run_ir(&d), Ok(Value::I32(7)), "DCE removed a live definition");
    assert_all_passes_preserve(&m);
}

#[test]
fn regalloc_register_defined_on_one_path_only() {
    // bb0: %0 = c; br %0 ? bb1 : bb2
    // bb1: %1 = 5; jmp bb3     bb2: %2 = 100; %3 = %2 + %2; jmp bb3
    // bb3: %4 = %1 + %1; ret %4   (c = false reads %1 uninitialised: 0)
    for (c, want) in [(true, 10), (false, 0)] {
        let m = module_of(vec![func(
            "main",
            vec![],
            5,
            vec![
                block(
                    0,
                    vec![Inst::LoadConst { dest: Reg(0), value: ConstValue::Bool(c) }],
                    Terminator::Branch { cond: Reg(0), then_bb: BlockId(1), else_bb: BlockId(2) },
                ),
                block(1, vec![konst(1, 5)], jump(3)),
                block(
                    2,
                    vec![konst(2, 100), bin(3, BinOp::Add, Type::I32, 2, 2)],
                    jump(3),
                ),
                block(3, vec![bin(4, BinOp::Add, Type::I32, 1, 1)], ret(4)),
            ],
        )]);
        assert_eq!(run_ir(&m), Ok(Value::I32(want)));
        assert_all_passes_preserve(&m);
    }
}

#[test]
fn regalloc_keeps_parameters_and_separates_types() {
    // add(p0: i32, p1: f64 unused, p2: i32): temporaries of different types
    // must not share a register, parameters never move.
    let f = func(
        "add3",
        vec![Type::I32, Type::F64, Type::I32],
        8,
        vec![block(
            0,
            vec![
                bin(3, BinOp::Add, Type::I32, 0, 2),
                Inst::LoadConst { dest: Reg(4), value: ConstValue::F64(1.5) },
                Inst::Cast { dest: Reg(5), src: Reg(3), from: Type::I32, to: Type::F64 },
                bin(6, BinOp::Add, Type::F64, 5, 4),
                Inst::Cast { dest: Reg(7), src: Reg(6), from: Type::F64, to: Type::I32 },
            ],
            ret(7),
        )],
    );
    let main = func(
        "main",
        vec![],
        5,
        vec![block(
            0,
            vec![
                konst(0, 20),
                Inst::LoadConst { dest: Reg(1), value: ConstValue::F64(0.5) },
                konst(2, 22),
                Inst::Call { dest: Some(Reg(3)), func: "add3".into(), args: vec![Reg(0), Reg(1), Reg(2)] },
            ],
            ret(3),
        )],
    );
    let m = module_of(vec![f, main]);
    assert_eq!(run_ir(&m), Ok(Value::I32(43)));
    let mut ra = m.clone();
    opt::regalloc::pass_regalloc(&mut ra);
    let f = ra.function("add3").unwrap();
    for (i, p) in f.params.iter().enumerate() {
        assert_eq!(p.2, Reg(i as u32));
    }
    assert_eq!(run_ir(&ra), Ok(Value::I32(43)));
    assert_all_passes_preserve(&m);
}

#[test]
fn regalloc_compacts_a_single_giant_block_of_temporaries() {
    // 30000 constants live one at a time in a single block (a big literal
    // array lowers to this shape): blocks are the unit of interference, so
    // this documents the current compaction behaviour and its limit.
    let n = 30000u32;
    let mut insts = vec![Inst::AllocArray { dest: Reg(0), elem: Type::I32, len: 3 }];
    insts.push(konst(1, 0));
    insts.push(konst(2, 1));
    for k in 0..n {
        insts.push(konst(3 + 2 * k, k as i32));
        insts.push(Inst::IndexStore { base: Reg(0), index: Reg(1), value: Reg(3 + 2 * k), elem: Type::I32 });
        insts.push(konst(4 + 2 * k, 1));
    }
    insts.push(Inst::IndexLoad { dest: Reg(3 + 2 * n), base: Reg(0), index: Reg(1), elem: Type::I32 });
    let m = module_of(vec![func("main", vec![], 4 + 2 * n, vec![block(0, insts, ret(3 + 2 * n))])]);
    assert_eq!(run_ir(&m), Ok(Value::I32(n as i32 - 1)));
    let (o, _) = optimize(m, 2);
    assert_eq!(run_ir(&o), Ok(Value::I32(n as i32 - 1)));
}

#[test]
fn inliner_never_grows_a_frame_past_the_vm_limit() {
    // callee: 40000 registers declared (one live), caller already at 30000:
    // inlining would need 70000 > 65535, so the call must stay a call.
    let callee = func(
        "leaf",
        vec![Type::I32],
        40000,
        vec![block(0, vec![bin(1, BinOp::Add, Type::I32, 0, 0)], ret(1))],
    );
    let main = func(
        "main",
        vec![],
        30000,
        vec![block(
            0,
            vec![
                konst(0, 21),
                Inst::Call { dest: Some(Reg(1)), func: "leaf".into(), args: vec![Reg(0)] },
            ],
            ret(1),
        )],
    );
    let mut m = module_of(vec![callee, main]);
    opt::inline::pass_inline(&mut m);
    assert!(m.function("main").unwrap().reg_count <= 65535);
    assert_eq!(run_ir(&m), Ok(Value::I32(42)));
    let (o, _) = optimize(m, 2);
    assert_eq!(run_ir(&o), Ok(Value::I32(42)));
}

#[test]
fn algebraic_pass_never_rewrites_float_identities() {
    // x - x, x == x, x * 0.0, x + 0.0 on f64 (NaN, inf, -0.0 are not identities)
    let nan = f64::NAN;
    let make = |op: BinOp, ty: Type, same: bool| {
        let rhs = if same { 0 } else { 1 };
        module_of(vec![func(
            "main",
            vec![],
            4,
            vec![block(
                0,
                vec![
                    Inst::LoadConst { dest: Reg(0), value: ConstValue::F64(nan) },
                    Inst::LoadConst { dest: Reg(1), value: ConstValue::F64(0.0) },
                    bin(2, op, ty, 0, rhs),
                ],
                ret(2),
            )],
        )])
    };
    for (op, same) in [
        (BinOp::Sub, true),
        (BinOp::Mul, false),
        (BinOp::Add, false),
        (BinOp::Eq, true),
        (BinOp::Le, true),
        (BinOp::Ne, true),
    ] {
        let m = make(op, Type::F64, same);
        let mut a = m.clone();
        opt::pass_algebraic(&mut a);
        assert_eq!(format!("{a:?}"), format!("{m:?}"), "{op:?} on f64 must stay untouched");
    }
    // the same shapes on i32 do rewrite
    let mi = module_of(vec![func(
        "main",
        vec![],
        3,
        vec![block(0, vec![konst(0, 9), bin(1, BinOp::Sub, Type::I32, 0, 0)], ret(1))],
    )]);
    let mut a = mi.clone();
    opt::pass_algebraic(&mut a);
    assert_ne!(a, mi);
    assert_eq!(run_ir(&a), Ok(Value::I32(0)));
}

#[test]
fn const_fold_matches_the_vm_on_integer_edges() {
    let cases: Vec<(BinOp, ConstValue, ConstValue)> = vec![
        (BinOp::Div, ConstValue::I32(i32::MIN), ConstValue::I32(-1)),
        (BinOp::Rem, ConstValue::I32(i32::MIN), ConstValue::I32(-1)),
        (BinOp::Div, ConstValue::I64(i64::MIN), ConstValue::I64(-1)),
        (BinOp::Rem, ConstValue::I64(i64::MIN), ConstValue::I64(-1)),
        (BinOp::Shl, ConstValue::I32(1), ConstValue::I32(32)),
        (BinOp::Shl, ConstValue::I32(1), ConstValue::I32(-1)),
        (BinOp::Shr, ConstValue::I32(-8), ConstValue::I32(33)),
        (BinOp::Shl, ConstValue::I64(1), ConstValue::I64(64)),
        (BinOp::Shl, ConstValue::I64(1), ConstValue::I64(-1)),
        (BinOp::Shr, ConstValue::I64(-8), ConstValue::I64(65)),
        (BinOp::Mul, ConstValue::I32(i32::MAX), ConstValue::I32(2)),
        (BinOp::Add, ConstValue::I64(i64::MAX), ConstValue::I64(1)),
        (BinOp::Lt, ConstValue::F64(f64::NAN), ConstValue::F64(1.0)),
        (BinOp::Ne, ConstValue::F64(f64::NAN), ConstValue::F64(f64::NAN)),
        (BinOp::Eq, ConstValue::F64(0.0), ConstValue::F64(-0.0)),
        (BinOp::Ge, ConstValue::F64(-0.0), ConstValue::F64(0.0)),
        (BinOp::Div, ConstValue::F64(1.0), ConstValue::F64(0.0)),
        (BinOp::Add, ConstValue::String("a".into()), ConstValue::String("é".into())),
        (BinOp::Lt, ConstValue::Char('a'), ConstValue::Char('b')),
        (BinOp::Ne, ConstValue::Bool(true), ConstValue::Bool(false)),
        (BinOp::Eq, ConstValue::String("x".into()), ConstValue::String("x".into())),
    ];
    for (op, l, r) in cases {
        let ty = l.ty();
        let rty = match op {
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => Type::Bool,
            _ => ty.clone(),
        };
        let mut m = module_of(vec![func(
            "main",
            vec![],
            3,
            vec![block(
                0,
                vec![
                    Inst::LoadConst { dest: Reg(0), value: l.clone() },
                    Inst::LoadConst { dest: Reg(1), value: r.clone() },
                    bin(2, op, ty, 0, 1),
                ],
                ret(2),
            )],
        )]);
        m.functions[0].return_ty = rty;
        // the VM, unfolded
        let bc = assemble(&m).unwrap_or_else(|e| panic!("{op:?} {l:?} {r:?}: {e}"));
        let want = aether::vm::execute_captured(&bc).map(|(v, _, _)| v).map_err(|e| e.to_string());
        let mut folded = m.clone();
        opt::pass_const_fold(&mut folded);
        let bc = assemble(&folded).unwrap();
        let got = aether::vm::execute_captured(&bc).map(|(v, _, _)| v).map_err(|e| e.to_string());
        match (&want, &got) {
            (Ok(Value::F64(a)), Ok(Value::F64(b))) => assert_eq!(a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan()), true, "{op:?}"),
            _ => assert_eq!(want, got, "{op:?} {l:?} {r:?}"),
        }
    }
}

#[test]
fn const_fold_leaves_division_by_zero_to_the_vm() {
    let m = module_of(vec![func(
        "main",
        vec![],
        3,
        vec![block(0, vec![konst(0, 1), konst(1, 0), bin(2, BinOp::Div, Type::I32, 0, 1)], ret(2))],
    )]);
    let mut f = m.clone();
    opt::pass_const_fold(&mut f);
    assert!(run_ir(&f).unwrap_err().contains("division by zero"));
    assert_all_passes_preserve(&m);
}

#[test]
fn dead_function_removal_keeps_externs_and_libraries() {
    let m = compile_source(
        "e.ae",
        "extern fn tick(); fn helper() { tick(); } fn main() -> i32 { tick(); return 0; }",
        &CompileOptions { opt_level: 2, color: false },
    );
    let ir = m.ir.unwrap();
    assert!(ir.function("tick").unwrap().is_extern, "extern kept");
    // a module with no main is a library: nothing is dropped
    let mut lib = module_of(vec![func("a", vec![], 1, vec![block(0, vec![konst(0, 1)], ret(0))])]);
    opt::pass_dead_functions(&mut lib);
    assert_eq!(lib.functions.len(), 1);
}

// ------------------------------------------------------------------ limits

fn bc_of(src: &str, level: u8) -> BytecodeModule {
    let c = compile_source("l.ae", src, &CompileOptions { opt_level: level, color: false });
    assert!(!c.diags.has_errors(), "{}", c.diags.render(&c.session, false));
    c.bytecode.expect("bytecode")
}

#[test]
fn stack_overflow_boundary_is_exact() {
    // main + r(n)..r(0) = n + 2 frames; limit L allows exactly L frames.
    let src = |n: i32| format!("fn r(n: i32) -> i32 {{ if n == 0 {{ return 0; }} return 1 + r(n - 1); }}\nfn main() -> i32 {{ return r({n}); }}");
    for level in [0u8, 2] {
        let opts = VmOptions { max_call_depth: 10, ..VmOptions::default() };
        let ok = bc_of(&src(8), level);
        assert_eq!(Vm::new(&ok, opts.clone()).run().unwrap(), Value::I32(8));
        let bad = bc_of(&src(9), level);
        assert!(matches!(Vm::new(&bad, opts.clone()).run().unwrap_err(), VmError::StackOverflow));
        // the same boundary through run_budget
        let mut vm = Vm::new(&bad, opts);
        let e = loop {
            match vm.run_budget(3) {
                Ok(Step::Yielded) => {}
                Ok(Step::Finished(v)) => panic!("finished with {v}"),
                Err(e) => break e,
            }
        };
        assert!(matches!(e, VmError::StackOverflow));
    }
    let tiny = bc_of("fn main() -> i32 { return 1; }", 0);
    let zero = VmOptions { max_call_depth: 0, ..VmOptions::default() };
    assert!(matches!(Vm::new(&tiny, zero).run().unwrap_err(), VmError::StackOverflow));
    let one = VmOptions { max_call_depth: 1, ..VmOptions::default() };
    assert_eq!(Vm::new(&tiny, one).run().unwrap(), Value::I32(1));
}

#[test]
fn step_limit_boundary_is_exact() {
    // `return 1;` is two instructions (loadimm, ret).
    let bc = bc_of("fn main() -> i32 { return 1; }", 0);
    for (limit, ok) in [(0u64, false), (1, false), (2, true), (3, true)] {
        let opts = VmOptions { max_steps: limit, ..VmOptions::default() };
        let mut vm = Vm::new(&bc, opts);
        let r = vm.run();
        assert_eq!(r.is_ok(), ok, "limit {limit}: {r:?}");
        if !ok {
            assert!(matches!(r.unwrap_err(), VmError::StepLimit));
        }
    }
}

// ----------------------------------------------------- run_budget semantics

#[derive(Clone)]
struct Capture(Rc<RefCell<Vec<u8>>>);

impl std::io::Write for Capture {
    fn write(&mut self, d: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(d);
        Ok(d.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// (outcome, stdout, steps, yields) of running `bc` with slices of `budget`
/// instructions (`None` = `run()`).
fn drive(bc: &BytecodeModule, budget: Option<u64>) -> (Result<Value, String>, String, u64, u64) {
    let cap = Capture(Rc::new(RefCell::new(Vec::new())));
    let mut vm = Vm::new(bc, VmOptions::default()).with_stdout(Box::new(cap.clone()));
    let mut yields = 0;
    let result = match budget {
        None => vm.run().map_err(|e| e.to_string()),
        Some(b) => loop {
            match vm.run_budget(b) {
                Ok(Step::Finished(v)) => break Ok(v),
                Ok(Step::Yielded) => yields += 1,
                Err(e) => break Err(e.to_string()),
            }
        },
    };
    let text = String::from_utf8_lossy(&cap.0.borrow()).into_owned();
    (result, text, vm.steps(), yields)
}

fn assert_budget_equals_run(bc: &BytecodeModule, what: &str) {
    let (want, want_out, want_steps, _) = drive(bc, None);
    for budget in [1u64, 2, 3, 7, 64, 1000] {
        let (got, got_out, got_steps, _) = drive(bc, Some(budget));
        assert_eq!(got, want, "{what}: budget {budget}");
        assert_eq!(got_out, want_out, "{what}: budget {budget}");
        assert_eq!(got_steps, want_steps, "{what}: budget {budget}");
    }
}

#[test]
fn run_budget_equals_run_for_every_example_and_stdlib_program() {
    let root = env!("CARGO_MANIFEST_DIR");
    let mut checked = 0;
    for dir in ["examples", "stdlib"] {
        let mut files: Vec<_> = std::fs::read_dir(format!("{root}/{dir}"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().map_or(false, |x| x == "ae"))
            .collect();
        files.sort();
        for path in files {
            for level in [0u8, 2] {
                let c = compile_file(path.to_str().unwrap(), &CompileOptions { opt_level: level, color: false })
                    .unwrap();
                if c.diags.has_errors() || c.bytecode.is_none() {
                    continue; // library without main, or needs --include
                }
                let bc = c.bytecode.as_ref().unwrap();
                if bc.functions[bc.entry as usize].is_native {
                    continue;
                }
                assert_budget_equals_run(bc, &format!("{} -O{level}", path.display()));
                checked += 1;
            }
        }
    }
    assert!(checked >= 20, "only {checked} programs checked");
}

#[test]
fn run_budget_equals_run_for_a_sample_of_the_corpus() {
    let root = env!("CARGO_MANIFEST_DIR");
    let mut files: Vec<_> = std::fs::read_dir(format!("{root}/corpus"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().map_or(false, |x| x == "ae"))
        .collect();
    files.sort();
    let mut checked = 0;
    for path in files.iter().step_by(11) {
        let c = compile_file(path.to_str().unwrap(), &CompileOptions { opt_level: 2, color: false }).unwrap();
        let Some(bc) = c.bytecode.as_ref().filter(|_| !c.diags.has_errors()) else { continue };
        assert_budget_equals_run(bc, &path.display().to_string());
        checked += 1;
    }
    assert!(checked >= 20, "only {checked} corpus programs checked");
}

const YIELDING: &str = r#"
    fn leaf(n: i32) -> i32 { yield; return n + 1; }
    fn tick() { yield; }
    fn mid(n: i32) -> i32 {
        let mut s = 0;
        let mut i = 0;
        while i < n { s = s + leaf(i); tick(); i = i + 1; }
        return s;
    }
    fn deep(n: i32) -> i32 { if n == 0 { yield; return 0; } return deep(n - 1) + 1; }
    fn main() -> i32 {
        print_i32(mid(4));
        yield;
        print_i32(deep(3));
        for i in 0..3 { for j in 0..2 { if (i + j) % 2 == 0 { yield; } } }
        return mid(2) + deep(2);
    }
"#;

#[test]
fn yield_inside_nested_calls_and_loops_resumes_exactly() {
    // yields: mid(4) 4*(leaf + tick) = 8, main 1, deep(3) 1, the loops 3
    // ((0,0), (1,1), (2,0)), mid(2) 4, deep(2) 1  =>  18. Value: mid(2) + deep(2) = 5.
    let mut yields_by_level = Vec::new();
    for level in [0u8, 2] {
        let bc = bc_of(YIELDING, level);
        let (whole, whole_out, _, _) = drive(&bc, None);
        assert_eq!(whole, Ok(Value::I32(5)), "-O{level}");
        assert_eq!(whole_out, "10\n3\n", "-O{level}");
        let mut counts = Vec::new();
        for budget in [1u64, 5, 1000, u64::MAX] {
            let (got, out, _, y) = drive(&bc, Some(budget));
            assert_eq!(got, whole, "-O{level} budget {budget}");
            assert_eq!(out, whole_out);
            counts.push((budget, y));
        }
        // with an unlimited budget only `yield` itself suspends the run
        let unlimited = counts.last().unwrap().1;
        assert_eq!(unlimited, 18, "-O{level}: yield count");
        yields_by_level.push(unlimited);
    }
    assert_eq!(yields_by_level[0], yields_by_level[1], "the optimizer must keep every yield");
}

#[test]
fn run_budget_zero_after_finish_and_after_error() {
    let bc = bc_of("fn main() -> i32 { print_i32(1); return 4; }", 0);
    let mut vm = Vm::new(&bc, VmOptions::default()).with_stdout(Box::new(std::io::sink()));
    // budget 0: no progress, no error
    assert_eq!(vm.run_budget(0).unwrap(), Step::Yielded);
    assert_eq!(vm.steps(), 0);
    assert!(!vm.is_finished());
    assert_eq!(vm.run_budget(1).unwrap(), Step::Yielded);
    assert_eq!(vm.steps(), 1);
    assert_eq!(vm.run_budget(u64::MAX).unwrap(), Step::Finished(Value::I32(4)));
    let steps = vm.steps();
    // after Finished: any budget, including 0, answers with the same value and counts nothing
    for b in [0u64, 1, 100] {
        assert_eq!(vm.run_budget(b).unwrap(), Step::Finished(Value::I32(4)));
    }
    assert_eq!(vm.run().unwrap(), Value::I32(4));
    assert_eq!(vm.steps(), steps);

    // after an error the VM stays halted, for run and run_budget alike
    let bad = bc_of("fn main() -> i32 { let z = 0; return 1 / z; }", 0);
    let mut vm = Vm::new(&bad, VmOptions::default());
    assert!(vm.run_budget(1_000).unwrap_err().to_string().contains("division by zero"));
    assert!(vm.run_budget(1_000).is_err());
    assert!(vm.run_budget(0).is_err());
    assert!(vm.run().is_err());
    assert!(!vm.is_finished());
}

// ------------------------------------------------------------- host API

#[test]
fn host_return_values_are_checked_against_the_extern_signature() {
    let script = |ret: &str| format!("extern fn f() -> {ret}; fn main() -> i32 {{ let x = f(); return 0; }}");
    let cases: Vec<(&str, Value, bool)> = vec![
        ("i32", Value::I32(1), true),
        ("i32", Value::Str("x".into()), false),
        ("i32", Value::I64(1), false),
        ("i32", Value::Unit, false),
        ("i64", Value::I64(1), true),
        ("i64", Value::I32(1), false),
        ("f64", Value::F64(1.5), true),
        ("f64", Value::I32(1), false),
        ("bool", Value::Bool(true), true),
        ("bool", Value::I32(1), false),
        ("string", Value::Str("s".into()), true),
        ("string", Value::Char('c'), false),
        ("char", Value::Char('c'), true),
        ("char", Value::I32(99), false),
        ("[i32; 2]", Value::array(vec![Value::I32(1), Value::I32(2)]), true),
        ("[i32; 2]", Value::array(vec![Value::I32(1)]), false),
        ("[i32; 2]", Value::array(vec![Value::I32(1), Value::Bool(true)]), false),
        ("[i32; 2]", Value::object(vec![Value::I32(1), Value::I32(2)]), false),
        ("(i32, bool)", Value::object(vec![Value::I32(1), Value::Bool(true)]), true),
        ("(i32, bool)", Value::object(vec![Value::I32(1), Value::I32(1)]), false),
        ("(i32, bool)", Value::object(vec![Value::I32(1)]), false),
    ];
    for (ty, v, ok) in cases {
        for level in [0u8, 2] {
            let v2 = v.clone();
            let r = Host::new().register("f", move |_| Ok(v2.clone())).eval("h.ae", &script(ty), level);
            if ok {
                r.unwrap_or_else(|e| panic!("{ty} <- {v:?} -O{level}: {e}"));
            } else {
                let e = r.unwrap_err();
                assert!(
                    e.contains("extern function `f` returned") && e.contains(&format!("declared to return `{ty}`")),
                    "{ty} <- {v:?} -O{level}: {e}"
                );
            }
        }
    }
}

#[test]
fn host_struct_and_enum_returns_are_checked() {
    let src = "struct P { x: i32, y: f64 } enum E { A(i32), B } \
               extern fn mk() -> P; extern fn me(n: i32) -> E; \
               fn main() -> i32 { let p = mk(); let e = me(0); match e { E::A(v) => { return p.x + v; } _ => { return p.x; } } }";
    let good = Host::new()
        .register("mk", |_| Ok(Value::object(vec![Value::I32(5), Value::F64(1.0)])))
        .register("me", |_| Ok(Value::object(vec![Value::I32(0), Value::I32(37)])))
        .eval("h.ae", src, 2)
        .unwrap();
    assert_eq!(good.value, Value::I32(42));
    let bad_field = Host::new()
        .register("mk", |_| Ok(Value::object(vec![Value::I32(5), Value::I32(1)])))
        .register("me", |_| Ok(Value::object(vec![Value::I32(1), Value::Unit])))
        .eval("h.ae", src, 2)
        .unwrap_err();
    assert!(bad_field.contains("returned an object of 2 fields but is declared to return `P`"), "{bad_field}");
    let bad_tag = Host::new()
        .register("mk", |_| Ok(Value::object(vec![Value::I32(5), Value::F64(1.0)])))
        .register("me", |_| Ok(Value::object(vec![Value::I32(9), Value::Unit])))
        .eval("h.ae", src, 0)
        .unwrap_err();
    assert!(bad_tag.contains("declared to return `E`"), "{bad_tag}");
}

#[test]
fn unit_extern_may_return_anything() {
    let r = Host::new()
        .register("note", |_| Ok(Value::Str("ignored".into())))
        .eval("u.ae", "extern fn note(x: i32); fn main() -> i32 { note(1); return 7; }", 2)
        .unwrap();
    assert_eq!(r.value, Value::I32(7));
}

#[test]
fn host_receives_arrays_structs_strings_and_tuples() {
    let log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    let r = Host::new()
        .register("see", move |args| {
            sink.borrow_mut().push(
                args.iter()
                    .map(|a| match a {
                        Value::Array(xs) => format!("arr{}", xs.len()),
                        Value::Object(xs) => format!("obj{}", xs.len()),
                        Value::Str(s) => format!("str:{s}"),
                        other => other.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(","),
            );
            Ok(Value::Unit)
        })
        .eval(
            "h.ae",
            r#"struct P { x: i32, y: i32 }
               extern fn see(a: [i32; 3], p: P, s: string, t: (i32, bool), c: char, n: f64);
               fn main() -> i32 { see([1, 2, 3], P { x: 1, y: 2 }, "héllo", (4, true), 'x', 2.5); return 0; }"#,
            2,
        )
        .unwrap();
    assert_eq!(r.value, Value::I32(0));
    assert_eq!(*log.borrow(), vec!["arr3,obj2,str:héllo,obj2,x,2.5".to_string()]);
}

#[test]
fn host_mutating_a_received_array_does_not_leak_back() {
    let r = Host::new()
        .register("poke", |args| {
            let mut v = args[0].clone();
            if let Value::Array(xs) = &mut v {
                Rc::make_mut(xs)[0] = Value::I32(99);
            }
            Ok(v)
        })
        .eval(
            "h.ae",
            "extern fn poke(a: [i32; 2]) -> [i32; 2]; \
             fn main() -> i32 { let a = [1, 2]; let b = poke(a); print_i32(a[0]); print_i32(b[0]); return 0; }",
            2,
        )
        .unwrap();
    assert_eq!(r.stdout, "1\n99\n");
}

#[test]
fn registering_the_same_name_twice_keeps_the_last_binding() {
    let r = Host::new()
        .register("v", |_| Ok(Value::I32(1)))
        .register("v", |_| Ok(Value::I32(2)))
        .eval("h.ae", "extern fn v() -> i32; fn main() -> i32 { return v(); }", 2)
        .unwrap();
    assert_eq!(r.value, Value::I32(2));
}

#[test]
fn host_errors_and_unbound_externs_are_clean_errors() {
    let e = Host::new()
        .register("boom", |_| Err(VmError::Runtime("host says no".into())))
        .eval("h.ae", "extern fn boom() -> i32; fn main() -> i32 { return boom(); }", 2)
        .unwrap_err();
    assert_eq!(e, "host says no");
    let e = Host::new()
        .eval("h.ae", "extern fn nope() -> i32; fn main() -> i32 { return nope(); }", 2)
        .unwrap_err();
    assert!(e.contains("extern function `nope` has no implementation"), "{e}");
}

#[test]
fn extern_calls_are_neither_removed_reordered_nor_merged() {
    // Calls whose results are unused or identical, inside a loop and next to
    // inlinable leaves, must all run, in program order, at every level.
    let src = r#"
        extern fn a(x: i32) -> i32;
        extern fn b(x: i32);
        fn leaf(x: i32) -> i32 { return x * 2; }
        fn main() -> i32 {
            let mut i = 0;
            let mut s = 0;
            while i < 4 {
                a(i);
                b(leaf(i));
                let same1 = a(7);
                let same2 = a(7);
                s = s + same1 - same2;
                i = i + 1;
            }
            b(a(1));
            return s;
        }
    "#;
    let mut logs = Vec::new();
    for level in [0u8, 1, 2] {
        let log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let (la, lb) = (log.clone(), log.clone());
        let counter = Rc::new(RefCell::new(0));
        let r = Host::new()
            .register("a", move |args| {
                let mut c = counter.borrow_mut();
                *c += 1;
                la.borrow_mut().push(format!("a({})", args[0].as_i32()));
                Ok(Value::I32(*c))
            })
            .register("b", move |args| {
                lb.borrow_mut().push(format!("b({})", args[0].as_i32()));
                Ok(Value::Unit)
            })
            .eval("h.ae", src, level)
            .unwrap();
        assert_eq!(r.value, Value::I32(-4), "-O{level}");
        let l = log.borrow().clone();
        assert_eq!(l.len(), 4 * 4 + 2, "-O{level}: {l:?}");
        logs.push(l);
    }
    assert_eq!(logs[0], logs[1]);
    assert_eq!(logs[0], logs[2]);
    assert_eq!(&logs[0][..4], ["a(0)", "b(0)", "a(7)", "a(7)"]);
    assert_eq!(&logs[0][16..], ["a(1)", "b(13)"]);
}

#[test]
fn extern_in_a_loop_with_unused_result_runs_every_iteration() {
    let src = "extern fn tick() -> i32; fn main() -> i32 { let mut i = 0; while i < 50 { tick(); i = i + 1; } return i; }";
    for level in [0u8, 2] {
        let n = Rc::new(RefCell::new(0));
        let m = n.clone();
        Host::new()
            .register("tick", move |_| {
                *m.borrow_mut() += 1;
                Ok(Value::I32(0))
            })
            .eval("h.ae", src, level)
            .unwrap();
        assert_eq!(*n.borrow(), 50, "-O{level}");
    }
}

#[test]
fn host_options_limit_the_script() {
    let mut h = Host::new();
    h.opts.max_call_depth = 5;
    let e = h
        .eval(
            "d.ae",
            "fn r(n: i32) -> i32 { if n == 0 { return 0; } return 1 + r(n - 1); } fn main() -> i32 { return r(50); }",
            0,
        )
        .unwrap_err();
    assert_eq!(e, VmError::StackOverflow.to_string());
}

// ------------------------------------------- assembler rejects malformed IR

fn ok_main() -> IrFunction {
    func("main", vec![], 1, vec![block(0, vec![konst(0, 1)], ret(0))])
}

fn assemble_err(m: &IrModule) -> String {
    assemble(m).expect_err("must be rejected")
}

#[test]
fn assembler_rejects_registers_outside_the_declared_count() {
    for inst in [
        konst(5, 1),
        bin(0, BinOp::Add, Type::I32, 0, 9),
        Inst::Move { dest: Reg(0), src: Reg(3) },
    ] {
        let mut f = ok_main();
        f.blocks[0].insts.push(inst);
        let e = assemble_err(&module_of(vec![f]));
        assert!(e.contains("outside the 1 declared registers"), "{e}");
    }
    let mut f = ok_main();
    f.blocks[0].term = ret(4);
    assert!(assemble_err(&module_of(vec![f])).contains("return value register %4"));
    let mut f = ok_main();
    f.blocks[0].term = Terminator::Branch { cond: Reg(2), then_bb: BlockId(0), else_bb: BlockId(0) };
    assert!(assemble_err(&module_of(vec![f])).contains("branch condition register %2"));
}

#[test]
fn assembler_rejects_bad_jumps_blocks_and_calls() {
    let mut f = ok_main();
    f.blocks[0].term = jump(7);
    assert!(assemble_err(&module_of(vec![f])).contains("missing block bb7"));

    let mut f = ok_main();
    f.blocks.push(block(0, vec![], ret(0)));
    assert!(assemble_err(&module_of(vec![f])).contains("two blocks numbered 0"));

    let mut f = ok_main();
    f.blocks.clear();
    assert!(assemble_err(&module_of(vec![f])).contains("has no blocks"));

    let callee = func("one", vec![Type::I32], 2, vec![block(0, vec![], ret(0))]);
    let mut f = ok_main();
    f.reg_count = 3;
    f.blocks[0].insts.push(Inst::Call { dest: Some(Reg(1)), func: "one".into(), args: vec![] });
    f.blocks[0].insts.push(Inst::Call { dest: Some(Reg(2)), func: "one".into(), args: vec![Reg(0), Reg(0)] });
    let e = assemble_err(&module_of(vec![callee.clone(), f]));
    assert!(e.contains("calls `one` with 0 arguments, it takes 1"), "{e}");

    let mut g = ok_main();
    g.reg_count = 2;
    g.blocks[0].insts.push(Inst::Call { dest: Some(Reg(1)), func: "ghost".into(), args: vec![] });
    assert_eq!(assemble_err(&module_of(vec![g])), "unknown function `ghost`");

    // the same function twice
    let e = assemble_err(&module_of(vec![ok_main(), ok_main()]));
    assert!(e.contains("duplicate function `main`"), "{e}");
}

#[test]
fn assembler_requires_parameters_in_the_first_registers() {
    let mut f = func("main", vec![Type::I32], 3, vec![block(0, vec![], ret(0))]);
    f.params[0].2 = Reg(2);
    assert!(assemble_err(&module_of(vec![f])).contains("parameter 0 lives in %2"));
}

#[test]
fn assembler_bounds_field_indices_counts_and_array_sizes() {
    let wide = Type::Tuple(vec![Type::I32; MAX_FIELDS + 1]);
    let mut f = ok_main();
    f.reg_count = 2;
    f.blocks[0].insts.push(Inst::AllocStruct { dest: Reg(1), ty: wide });
    assert!(assemble_err(&module_of(vec![f])).contains("field count 65536 exceeds"));

    let mut f = ok_main();
    f.reg_count = 2;
    f.blocks[0].insts.push(Inst::FieldLoad { dest: Reg(1), base: Reg(0), index: 70_000, ty: Type::I32 });
    assert!(assemble_err(&module_of(vec![f])).contains("field index 70000 exceeds"));

    // exactly the largest object is fine
    let max = Type::Tuple(vec![Type::I32; MAX_FIELDS]);
    let mut f = ok_main();
    f.reg_count = 2;
    f.blocks[0].insts.push(Inst::AllocStruct { dest: Reg(1), ty: max });
    f.blocks[0].insts.push(Inst::FieldLoad { dest: Reg(0), base: Reg(1), index: MAX_FIELDS - 1, ty: Type::I32 });
    assemble(&module_of(vec![f])).expect("65535 fields assemble");

    for len in [-1i64, 1 << 40, (1 << 28) + 1] {
        let mut f = ok_main();
        f.reg_count = 2;
        f.blocks[0].insts.push(Inst::AllocArray { dest: Reg(1), elem: Type::I32, len });
        let e = assemble_err(&module_of(vec![f]));
        assert!(e.contains("exceeds the VM limit"), "{len}: {e}");
    }

    let mut f = ok_main();
    f.reg_count = 2;
    f.blocks[0].insts.push(Inst::AllocStruct { dest: Reg(1), ty: Type::I32 });
    assert!(assemble_err(&module_of(vec![f])).contains("cannot allocate an object of type `i32`"));
}

#[test]
fn assembler_refuses_functions_needing_more_than_65535_registers() {
    let mut f = ok_main();
    f.reg_count = 65_536;
    let e = assemble_err(&module_of(vec![f]));
    assert_eq!(e, "function `main` needs 65536 registers; the VM supports at most 65535");
    let mut f = ok_main();
    f.reg_count = 65_535;
    f.blocks[0].insts.push(konst(65_534, 3));
    assemble(&module_of(vec![f])).expect("the largest frame assembles");
}

#[test]
fn compile_error_for_malformed_programs_is_e0300_not_a_panic() {
    // 40000 literal elements in one block exceed the VM's register space.
    let n = 40_000;
    let mut src = String::from("fn main() -> i32 {\n let mut a = [");
    src.push_str(&vec!["1"; n].join(", "));
    src.push_str("];\n return a[0];\n}\n");
    for level in [0u8] {
        let c = compile_source("big.ae", &src, &CompileOptions { opt_level: level, color: false });
        assert!(c.diags.has_errors(), "-O{level}");
        let text = c.diags.render(&c.session, false);
        assert!(text.contains("E0300") && text.contains("registers"), "-O{level}: {text}");
    }
}

// ------------------------------------------- VM refuses type-confused code

fn tiny(code: Vec<Op>, nregs: u16) -> BytecodeModule {
    BytecodeModule {
        functions: vec![BcFunction {
            name: "main".into(),
            arity: 0,
            nregs,
            code,
            is_native: false,
            native_id: None,
            ret_ty: None,
        }],
        strings: vec!["abc".into()],
        entry: 0,
    }
}

fn vm_error(code: Vec<Op>, nregs: u16) -> String {
    Vm::new(&tiny(code, nregs), VmOptions::default())
        .run()
        .expect_err("must fail")
        .to_string()
}

#[test]
fn vm_reports_field_access_on_the_wrong_kind_of_value() {
    let unit = Op::LoadImm { dest: 0, imm: Immediate::Unit };
    let e = vm_error(vec![unit.clone(), Op::LoadField { dest: 1, base: 0, field: 0 }, Op::RetVoid], 2);
    assert_eq!(e, "cannot read field 0 of a value of type unit");
    let e = vm_error(vec![unit.clone(), Op::StoreField { base: 0, field: 3, value: 0 }, Op::RetVoid], 2);
    assert_eq!(e, "cannot write field 3 of a value of type unit");
    let e = vm_error(
        vec![Op::AllocObj { dest: 0, fields: 2 }, Op::LoadField { dest: 1, base: 0, field: 2 }, Op::RetVoid],
        2,
    );
    assert!(e.contains("cannot read field 2: the object has 2 fields"), "{e}");
    let e = vm_error(
        vec![
            Op::AllocObj { dest: 0, fields: 2 },
            Op::StoreField { base: 0, field: 9, value: 0 },
            Op::RetVoid,
        ],
        1,
    );
    assert!(e.contains("cannot write field 9"), "{e}");
}

#[test]
fn vm_reports_indexing_the_wrong_kind_of_value() {
    let int = Op::LoadImm { dest: 0, imm: Immediate::I32(3) };
    let e = vm_error(vec![int.clone(), Op::LoadIdx { dest: 1, base: 0, index: 0 }, Op::RetVoid], 2);
    assert_eq!(e, "cannot index a value of type i32");
    let e = vm_error(vec![int.clone(), Op::StoreIdx { base: 0, index: 0, value: 0 }, Op::RetVoid], 1);
    assert_eq!(e, "cannot store into a value of type i32");
    // index operand must be an i32
    let e = vm_error(
        vec![
            Op::AllocArr { dest: 0, len: 2 },
            Op::LoadImm { dest: 1, imm: Immediate::I64(0) },
            Op::LoadIdx { dest: 2, base: 0, index: 1 },
            Op::RetVoid,
        ],
        3,
    );
    assert_eq!(e, "index must be i32, got i64");
}

#[test]
fn vm_concat_and_natives_check_operand_types() {
    let e = vm_error(
        vec![
            Op::LoadStr { dest: 0, idx: 0 },
            Op::LoadImm { dest: 1, imm: Immediate::I32(1) },
            Op::Concat { dest: 2, lhs: 0, rhs: 1 },
            Op::RetVoid,
        ],
        3,
    );
    assert_eq!(e, "cannot concatenate string with i32");
    let e = vm_error(
        vec![
            Op::LoadImm { dest: 0, imm: Immediate::Unit },
            Op::CallNative { id: 2, dest: None, args: vec![0] },
            Op::RetVoid,
        ],
        1,
    );
    assert_eq!(e, "`print_i32` expects i32 for argument 1, got unit");
    let e = vm_error(
        vec![
            Op::LoadImm { dest: 0, imm: Immediate::I32(1) },
            Op::CallNative { id: 6, dest: Some(1), args: vec![0] },
            Op::RetVoid,
        ],
        2,
    );
    assert_eq!(e, "`len` expects string or array for argument 1, got i32");
    let e = vm_error(vec![Op::CallNative { id: 13, dest: Some(0), args: vec![] }, Op::RetVoid], 1);
    assert_eq!(e, "`abs` expects i32 for argument 1, got nothing");
}

#[test]
fn vm_array_allocation_is_capped() {
    let e = vm_error(vec![Op::AllocArr { dest: 0, len: u32::MAX }, Op::RetVoid], 1);
    assert!(e.contains("exceeds the limit"), "{e}");
}

#[test]
fn vm_compares_aggregates_elementwise_and_refuses_ordering() {
    let eq = |op: aether::backend::bytecode::CmpOp| {
        let code = vec![
            Op::AllocArr { dest: 0, len: 2 },
            Op::AllocArr { dest: 1, len: 2 },
            Op::Cmp { op, dest: 2, lhs: 0, rhs: 1 },
            Op::Ret { src: 2 },
        ];
        Vm::new(&tiny(code, 3), VmOptions::default()).run()
    };
    use aether::backend::bytecode::CmpOp;
    assert_eq!(eq(CmpOp::Eq).unwrap(), Value::Bool(true));
    assert_eq!(eq(CmpOp::Ne).unwrap(), Value::Bool(false));
    assert!(eq(CmpOp::Lt).unwrap_err().to_string().contains("cannot order two values of type array"));
}

#[test]
fn disassembly_names_jumps_by_what_they_test() {
    let bc = bc_of("fn main() -> i32 { let mut i = 0; while i < 3 { i = i + 1; } return i; }", 0);
    let text = bc.disassemble();
    // `JumpIf` jumps when the register is true: "jnz", never "jz"
    assert!(text.contains("jnz r"), "{text}");
    assert!(!text.contains("jz r"), "{text}");
}

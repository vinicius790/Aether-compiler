//! Adversarial audit of `match` (nested patterns, tuple patterns, `match`
//! as an expression, exhaustiveness), `let` destructuring, compound
//! assignment, bitwise operators, literal forms and the built-ins.
//!
//! Every runnable program is executed at -O0 and -O2 on the VM; both must
//! produce exactly the recorded stdout. Expected values were derived by hand
//! from `docs/language.md`.

use aether::{compile_source, run_source, CompileOptions};

fn compile_diags(src: &str) -> (bool, String) {
    let opts = CompileOptions {
        opt_level: 0,
        color: false,
    };
    let c = compile_source("audit.ae", src, &opts);
    (c.diags.has_errors(), c.diags.render(&c.session, false))
}

const RUN: &[(&str, &str, &str)] = &[
    ("array_equality", r##"enum E { A(i32), B }
struct S { a: [i32; 2], t: (i32, bool) }
fn main() -> i32 {
    print_bool([1, 2] == [1, 2]);
    print_bool([1, 2] == [1, 3]);
    print_bool([[1], [2]] == [[1], [2]]);
    print_bool([(1, 2)] != [(1, 2)]);
    print_bool([E::A(1), E::B] == [E::A(1), E::B]);
    print_bool([E::A(1)] == [E::B]);
    let n = 0.0 / 0.0;
    print_bool([n] == [n]);
    let s = S { a: [1, 2], t: (3, true) };
    let u = S { a: [1, 2], t: (3, true) };
    print_bool(s == u);
    print_bool((s.a, 1) == (u.a, 1));
    let mut v = u;
    v.a[1] = 5;
    print_bool(s == v);
    print_bool(s != v);
    print_bool(["a", "b"] == ["a", "b"]);
    return 0;
}
"##, r##"true
false
true
false
true
false
false
true
true
false
true
true
"##),
    ("dead_value_match_default", r##"fn main() -> i32 {
    let a = match 1 { 1 => (1, 2), _ => (3, 4) };
    let b = match 2 { 1 => [1, 2], _ => [3, 4] };
    let c = match 1 { 1 => "s", _ => "t" };
    let d = match 1 { 1 => 1.5, _ => 2.5 };
    let e = match 1 { 1 => 'x', _ => 'y' };
    let f = match 3 { 1 => true, _ => false };
    print_i32(a.0 + b[0]); println(c); print_f64(d); print_char(e); print_bool(f);
    return 0;
}
"##, r##"4
s
1.5
x
false
"##),
    ("match_expr_unit_valued", r##"fn say(n: i32) { print_i32(n); }
fn main() -> i32 {
    let u = match 1 { 1 => say(7), _ => say(8) };
    return 0;
}
"##, r##"7
"##),
    ("deep_pattern_with_literals_and_tuples", r##"enum E { P((i32, bool), i32), Q }
fn f(e: E) -> i32 {
    match e {
        E::P((0, true), x) => x,
        E::P((n, false), 5) => n,
        E::P((_, _), _) => -1,
        E::Q => -2,
    }
}
fn main() -> i32 {
    print_i32(f(E::P((0, true), 9)));
    print_i32(f(E::P((4, false), 5)));
    print_i32(f(E::P((4, false), 6)));
    print_i32(f(E::P((1, true), 5)));
    print_i32(f(E::Q));
    return 0;
}
"##, r##"9
4
-1
-1
-2
"##),
    ("nested_variant", r##"enum In { A(i32), B }
enum Out { W(In), N }
fn f(o: Out) -> i32 {
    match o {
        Out::W(In::A(x)) => x,
        Out::W(In::B) => 100,
        Out::N => 200,
    }
}
fn main() -> i32 {
    print_i32(f(Out::W(In::A(7))));
    print_i32(f(Out::W(In::B)));
    print_i32(f(Out::N));
    return 0;
}
"##, r##"7
100
200
"##),
    ("literal_in_variant", r##"enum E { A(i32), B }
fn g(e: E) -> i32 {
    match e { E::A(1) => 10, E::A(_) => 20, E::B => 30 }
}
fn main() -> i32 {
    print_i32(g(E::A(1))); print_i32(g(E::A(5))); print_i32(g(E::B));
    return 0;
}
"##, r##"10
20
30
"##),
    ("tuple_pattern_match", r##"fn main() -> i32 {
    let t = (1, true);
    match t {
        (0, _) => println("zero"),
        (1, true) => println("one-true"),
        (_, _) => println("other"),
    }
    let u = (2, false);
    match u {
        (1, true) => println("a"),
        (a, b) => { print_i32(a); print_bool(b); }
    }
    return 0;
}
"##, r##"one-true
2
false
"##),
    ("bool_match_exhaustive", r##"fn f(b: bool) -> i32 { match b { true => 1, false => 0 } }
fn main() -> i32 { print_i32(f(true)); print_i32(f(false)); return 0; }
"##, r##"1
0
"##),
    ("let_nested_tuple", r##"fn main() -> i32 {
    let t = (1, (2, 3), (true, "x"));
    let (a, (b, c), (d, _)) = t;
    print_i32(a + b + c);
    print_bool(d);
    return 0;
}
"##, r##"6
true
"##),
    ("let_nested_mut", r##"fn main() -> i32 {
    let t = (1, (2, 3));
    let mut (a, (b, c)) = t;
    a = a + 10; b += 20; c *= 3;
    print_i32(a); print_i32(b); print_i32(c);
    print_i32(t.1.0);
    return 0;
}
"##, r##"11
22
9
2
"##),
    ("match_expr_let", r##"enum E { A(i32), B }
fn main() -> i32 {
    let e = E::A(4);
    let a = match e { E::A(x) => x * 2, E::B => 0 };
    let b = match E::B { E::A(x) => x, E::B => -1 };
    print_i32(a); print_i32(b);
    return 0;
}
"##, r##"8
-1
"##),
    ("match_expr_block_arms", r##"fn main() -> i32 {
    let n = 5;
    let r = match n {
        0 => 100,
        5 => { let k = n * 2; k + 1 }
        _ => { 7 }
    };
    print_i32(r);
    return 0;
}
"##, r##"11
"##),
    ("match_diverging_arm_return", r##"fn f(n: i32) -> i32 {
    let v = match n { 0 => return 99, k => k + 1 };
    return v * 2;
}
fn main() -> i32 { print_i32(f(0)); print_i32(f(3)); return 0; }
"##, r##"99
8
"##),
    ("match_diverging_arm_block_return", r##"fn f(n: i32) -> i32 {
    let v = match n { 0 => { return 99; } k => k + 1 };
    return v * 2;
}
fn main() -> i32 { print_i32(f(0)); print_i32(f(3)); return 0; }
"##, r##"99
8
"##),
    ("match_in_loop_break_continue", r##"fn main() -> i32 {
    let mut s = 0;
    for i in 0..10 {
        let v = match i % 3 {
            0 => continue,
            1 => i,
            _ => { if i > 6 { break; } i * 10 }
        };
        s += v;
    }
    print_i32(s);
    return 0;
}
"##, r##"82
"##),
    ("match_tail_returns_value", r##"enum Shape { Circle(i32), Rect(i32, i32), Empty }
fn area(s: Shape) -> i32 {
    match s {
        Shape::Circle(r) => 3 * r * r,
        Shape::Rect(w, h) => w * h,
        Shape::Empty => 0,
    }
}
fn main() -> i32 { print_i32(area(Shape::Circle(2))); print_i32(area(Shape::Rect(3, 4))); print_i32(area(Shape::Empty)); return 0; }
"##, r##"12
12
0
"##),
    ("match_tail_block_arms", r##"fn f(n: i32) -> i32 {
    match n {
        0 => { 10 }
        1 => { let a = 5; a + a }
        _ => { return 77; }
    }
}
fn main() -> i32 { print_i32(f(0)); print_i32(f(1)); print_i32(f(2)); return 0; }
"##, r##"10
10
77
"##),
    ("match_stmt_unchanged", r##"fn main() -> i32 {
    let x = 2;
    match x {
        1 => { println("one"); }
        2 => { println("two"); }
        _ => { }
    }
    match x { 2 => { println("again"); } _ => {} }
    return 0;
}
"##, r##"two
again
"##),
    ("match_stmt_tail_unit_fn", r##"fn f(x: i32) {
    match x { 1 => { println("one"); } _ => { println("other"); } }
}
fn main() -> i32 { f(1); f(2); return 0; }
"##, r##"one
other
"##),
    ("match_stmt_tail_nonunit_arms_ok", r##"fn f(x: i32) {
    match x { 1 => { to_string(5) } _ => { 7 } }
}
fn main() -> i32 { f(1); println("done"); return 0; }
"##, r##"done
"##),
    ("scrutinee_once", r##"fn tick(c: i32) -> i32 { print_i32(c); return c; }
fn main() -> i32 {
    match tick(2) { 1 => { println("a"); } 2 => { println("b"); } _ => { println("c"); } }
    let r = match tick(3) { 3 => 30, _ => 0 };
    print_i32(r);
    return 0;
}
"##, r##"2
b
3
30
"##),
    ("match_shadow", r##"fn main() -> i32 {
    let x = 10;
    let r = match x { x => x + 1 };
    print_i32(r); print_i32(x);
    let t = (1, 2);
    let q = match t { (x, y) => x * 10 + y };
    print_i32(q);
    return 0;
}
"##, r##"11
10
12
"##),
    ("nested_match_expr", r##"enum E { A(i32), B }
fn main() -> i32 {
    let p = (E::A(2), E::B);
    let v = match p {
        (E::A(x), E::A(y)) => x + y,
        (E::A(x), E::B) => match x { 2 => 20, _ => 21 },
        (E::B, _) => 0,
    };
    print_i32(v);
    return 0;
}
"##, r##"20
"##),
    ("match_expr_in_binary_alias", r##"fn main() -> i32 {
    let mut x = 1;
    let r = x + match 0 { 0 => { x = 10; 5 } _ => 0 };
    print_i32(r); print_i32(x);
    return 0;
}
"##, r##"6
10
"##),
    ("match_expr_in_call_alias", r##"fn add(a: i32, b: i32) -> i32 { return a * 100 + b; }
fn main() -> i32 {
    let mut x = 1;
    let r = add(x, match 0 { 0 => { x = 10; 5 } _ => 0 });
    print_i32(r); print_i32(x);
    return 0;
}
"##, r##"105
10
"##),
    ("i64_arm_inference", r##"fn main() -> i32 {
    let big: i64 = 5000000000;
    let k = 1;
    let r = match k { 1 => 2, _ => big };
    print_i64(r);
    let s = match k { 1 => big, _ => 3 };
    print_i64(s);
    return 0;
}
"##, r##"2
5000000000
"##),
    ("match_string_scrutinee", r##"fn main() -> i32 {
    let s = "hi";
    let n = match s { "hi" => 1, "yo" => 2, _ => 3 };
    print_i32(n);
    let c = 'x';
    let m = match c { 'a' => 1, 'x' => 2, _ => 3 };
    print_i32(m);
    return 0;
}
"##, r##"1
2
"##),
    ("return_match", r##"enum E { A(i32), B }
fn f(e: E) -> i32 { return match e { E::A(x) => x, E::B => -1 }; }
fn main() -> i32 { print_i32(f(E::A(3))); print_i32(f(E::B)); return 0; }
"##, r##"3
-1
"##),
    ("match_arg_expr", r##"fn id(x: i32) -> i32 { return x; }
fn main() -> i32 {
    print_i32(id(match 3 { 3 => 33, _ => 0 }) + 1);
    return 0;
}
"##, r##"34
"##),
    ("if_let_nested", r##"enum In { A(i32), B }
enum Out { W(In), N }
fn main() -> i32 {
    let o = Out::W(In::A(5));
    if let Out::W(In::A(x)) = o { print_i32(x); } else { println("no"); }
    if let Out::W(In::B) = o { println("b"); } else { println("not b"); }
    if let (1, y) = (1, 9) { print_i32(y); }
    return 0;
}
"##, r##"5
not b
9
"##),
    ("deep_nesting_pattern", r##"enum A { X(B) }
enum B { Y(C) }
enum C { Z(i32), W }
fn f(a: A) -> i32 { match a { A::X(B::Y(C::Z(n))) => n, A::X(B::Y(C::W)) => -1 } }
fn main() -> i32 { print_i32(f(A::X(B::Y(C::Z(8))))); print_i32(f(A::X(B::Y(C::W)))); return 0; }
"##, r##"8
-1
"##),
    ("single_variant_enum", r##"enum One { O(i32) }
fn f(o: One) -> i32 { match o { One::O(x) => x + 1 } }
fn main() -> i32 { print_i32(f(One::O(4))); return 0; }
"##, r##"5
"##),
    ("enum_in_array_mutate", r##"enum E { A(i32), B }
fn main() -> i32 {
    let mut a = [E::A(1), E::B, E::A(3)];
    a[1] = E::A(9);
    let b = a;
    a[0] = E::B;
    let mut i = 0;
    while i < 3 {
        match a[i] { E::A(x) => print_i32(x), E::B => println("B") }
        i += 1;
    }
    match b[0] { E::A(x) => print_i32(x), E::B => println("B") }
    return 0;
}
"##, r##"B
9
3
1
"##),
    ("enum_in_struct", r##"enum E { A(i32), B }
struct S { e: E, n: i32 }
fn bump(s: S) -> S { let mut t = s; t.e = E::A(t.n); t.n += 1; return t; }
fn main() -> i32 {
    let s = S { e: E::B, n: 4 };
    let t = bump(s);
    match s.e { E::A(_) => println("A"), E::B => println("B") }
    match t.e { E::A(x) => print_i32(x), E::B => println("B") }
    print_i32(t.n);
    return 0;
}
"##, r##"B
4
5
"##),
    ("enum_in_tuple", r##"enum E { A(i32), B }
fn main() -> i32 {
    let mut t = (E::A(1), 5);
    let u = t;
    t.0 = E::B;
    t.1 = 6;
    match t.0 { E::A(x) => print_i32(x), E::B => println("B") }
    match u.0 { E::A(x) => print_i32(x), E::B => println("B") }
    print_i32(u.1);
    return 0;
}
"##, r##"B
1
5
"##),
    ("recursion_with_enum", r##"enum Op { Add(i32), Mul(i32), Stop }
fn run(op: Op, acc: i32, n: i32) -> i32 {
    if n == 0 { return acc; }
    match op {
        Op::Add(k) => run(Op::Mul(k), acc + k, n - 1),
        Op::Mul(k) => run(Op::Add(k + 1), acc * k, n - 1),
        Op::Stop => acc,
    }
}
fn main() -> i32 { print_i32(run(Op::Add(2), 1, 4)); print_i32(run(Op::Stop, 7, 3)); return 0; }
"##, r##"27
7
"##),
    ("empty_payload_eq", r##"enum E { A, B, C(i32) }
fn main() -> i32 {
    print_bool(E::A == E::A);
    print_bool(E::A == E::B);
    print_bool(E::C(1) == E::C(1));
    print_bool(E::C(1) == E::C(2));
    print_bool(E::A != E::C(0));
    return 0;
}
"##, r##"true
false
true
false
true
"##),
    ("tuple_eq_nan", r##"fn main() -> i32 {
    let n = 0.0 / 0.0;
    let a = (n, 1);
    let b = (n, 1);
    print_bool(a == b);
    print_bool(a != b);
    print_bool(((1, 2), 3) == ((1, 2), 3));
    print_bool(((1, 2), 3) == ((1, 9), 3));
    return 0;
}
"##, r##"false
true
true
false
"##),
    ("tuple_eq_enum", r##"enum E { A(i32), B }
fn main() -> i32 {
    print_bool((E::A(1), 2) == (E::A(1), 2));
    print_bool((E::A(1), 2) == (E::B, 2));
    print_bool((E::B, (1, 2.5)) == (E::B, (1, 2.5)));
    return 0;
}
"##, r##"true
false
true
"##),
    ("compound_double_eval", r##"fn f(c: i32) -> i32 { print_i32(c); return 0; }
fn main() -> i32 {
    let mut a = [10, 20];
    a[f(1)] += 5;
    print_i32(a[0]);
    return 0;
}
"##, r##"1
1
15
"##),
    ("compound_struct_in_tuple", r##"struct P { x: i32, y: i32 }
fn main() -> i32 {
    let mut t = (P { x: 1, y: 2 }, 5);
    t.0.x += 10;
    t.0.y *= 7;
    t.1 -= 2;
    print_i32(t.0.x); print_i32(t.0.y); print_i32(t.1);
    return 0;
}
"##, r##"11
14
3
"##),
    ("compound_all_ops", r##"fn main() -> i32 {
    let mut x = 100;
    x += 5; print_i32(x);
    x -= 3; print_i32(x);
    x *= 2; print_i32(x);
    x /= 4; print_i32(x);
    x %= 7; print_i32(x);
    x <<= 3; print_i32(x);
    x >>= 1; print_i32(x);
    x |= 1; print_i32(x);
    x &= 6; print_i32(x);
    x ^= 5; print_i32(x);
    return 0;
}
"##, r##"105
102
204
51
2
16
8
9
0
5
"##),
    ("compound_string", r##"fn main() -> i32 {
    let mut s = "a";
    s += "b";
    s += to_string(1);
    println(s);
    return 0;
}
"##, r##"ab1
"##),
    ("compound_rhs_whole", r##"fn main() -> i32 {
    let mut x = 3;
    x *= 2 + 3;
    print_i32(x);
    let mut y = 10;
    y -= 1 - 4;
    print_i32(y);
    return 0;
}
"##, r##"15
13
"##),
    ("shifts_masked", r##"fn main() -> i32 {
    print_i32(1 << 32);
    print_i32(1 << 31);
    print_i32(1 << -1);
    print_i32(-16 >> 2);
    print_i32(-1 >> 40);
    print_i32(256 >> 33);
    let a: i64 = 1;
    print_i64(a << 64);
    print_i64(a << 63);
    print_i64(a << -1);
    return 0;
}
"##, r##"1
-2147483648
-2147483648
-4
-1
128
1
-9223372036854775808
-9223372036854775808
"##),
    ("not_bitwise_i64", r##"fn main() -> i32 {
    let a: i64 = 5;
    print_i64(!a);
    print_i64(!0 as i64);
    print_i32(!0);
    print_i32(!5);
    print_bool(!true);
    let z: i64 = 0;
    print_i64(!z);
    return 0;
}
"##, r##"-6
-1
-1
-6
false
-1
"##),
    ("int_literals", r##"fn main() -> i32 {
    print_i32(0x7FFFFFFF);
    print_i32(1_000);
    print_i32(0b1010);
    print_i32(0o17);
    print_i32(0xFF_FF);
    let a: i64 = 0x80000000;
    print_i64(a);
    print_i64(0xFFFF_FFFF);
    return 0;
}
"##, r##"2147483647
1000
10
15
65535
2147483648
4294967295
"##),
    ("literal_min_i32", r##"fn main() -> i32 { let a = -2147483648; print_i32(a); let b: i64 = -9223372036854775807; print_i64(b); return 0; }
"##, r##"-2147483648
-9223372036854775807
"##),
    ("unicode_escapes", r##"fn main() -> i32 {
    let s = "\u{41}\u{1F600}b";
    print_i32(len(s));
    print_char(s[0]);
    print_i32(s[1] as i32);
    let c = '\u{e9}';
    print_i32(c as i32);
    println("tab\there");
    return 0;
}
"##, r##"3
A
128512
233
tab	here
"##),
    ("len_forms", r##"fn main() -> i32 {
    let a = [1, 2, 3];
    print_i32(len(a));
    print_i32(len("héllo"));
    print_i32(len(""));
    let m = [[1, 2], [3, 4], [5, 6]];
    print_i32(len(m)); print_i32(len(m[0]));
    return 0;
}
"##, r##"3
5
0
3
2
"##),
    ("to_string_forms", r##"fn main() -> i32 {
    println(to_string(-5));
    println(to_string(0));
    println(i64_to_string(-9223372036854775807));
    println(f64_to_string(0.0 / 0.0));
    println(f64_to_string(-0.5));
    println(f64_to_string(2.0));
    println(char_to_string('z'));
    println(to_string(-2147483648));
    return 0;
}
"##, r##"-5
0
-9223372036854775807
NaN
-0.5
2
z
-2147483648
"##),
    ("pow_overflow", r##"fn main() -> i32 {
    print_i32(pow_i32(2, 31));
    print_i32(pow_i32(2, 32));
    print_i32(pow_i32(3, 20));
    print_i32(pow_i32(5, 0));
    print_i32(pow_i32(5, -1));
    print_i32(pow_i32(0, 0));
    print_i32(pow_i32(-2, 3));
    return 0;
}
"##, r##"-2147483648
0
-808182895
1
0
1
-8
"##),
    ("abs_min", r##"fn main() -> i32 {
    print_i32(abs(-2147483648));
    print_i32(abs(-5));
    print_i32(abs(0));
    print_i32(min(3, -3)); print_i32(max(3, -3));
    return 0;
}
"##, r##"-2147483648
5
0
-3
3
"##),
    ("clamp_inverted", r##"fn main() -> i32 {
    print_i32(clamp(5, 10, 0));
    print_i32(clamp(-5, 10, 0));
    print_i32(clamp(15, 10, 0));
    print_i32(clamp(5, 0, 10));
    print_i32(clamp(-5, 0, 10));
    print_i32(clamp(50, 0, 10));
    return 0;
}
"##, r##"10
10
10
5
0
10
"##),
    ("sqrt_floor_ceil", r##"fn main() -> i32 {
    print_f64(sqrt(16.0));
    print_f64(floor(-1.5));
    print_f64(ceil(-1.5));
    print_f64(floor(2.0));
    print_f64(sqrt(-1.0));
    return 0;
}
"##, r##"4
-2
-1
2
NaN
"##),
    ("div_min_neg1", r##"fn main() -> i32 {
    let a = -2147483647 - 1;
    let b = -1;
    print_i32(a / b);
    print_i32(a % b);
    return 0;
}
"##, r##"-2147483648
0
"##),
    ("unreachable_after_return", r##"fn f(x: i32) -> i32 {
    match x {
        0 => { return 1; println("dead"); }
        _ => { return 2; }
    }
}
fn main() -> i32 { print_i32(f(0)); print_i32(f(1)); return 0; }
"##, r##"1
2
"##),
    ("tail_match_with_stmts_before", r##"fn f(n: i32) -> i32 {
    let m = n * 2;
    match m { 4 => 1, _ => 0 }
}
fn main() -> i32 { print_i32(f(2)); print_i32(f(3)); return 0; }
"##, r##"1
0
"##),
    ("deep_nesting_ok", r##"fn main() -> i32 {
    let x = ((((((((((((((((((((1))))))))))))))))))));
    print_i32(x);
    return 0;
}
"##, r##"1
"##),
    ("match_in_while_cond_like", r##"fn main() -> i32 {
    let mut i = 0;
    while match i { 3 => false, _ => true } { i += 1; }
    print_i32(i);
    return 0;
}
"##, r##"3
"##),
    ("match_in_for_bounds", r##"fn main() -> i32 {
    let mut s = 0;
    for i in match 1 { 1 => 2, _ => 0 } .. match 1 { 1 => 5, _ => 0 } { s += i; }
    print_i32(s);
    return 0;
}
"##, r##"9
"##),
    ("for_bound_once", r##"fn main() -> i32 {
    let mut n = 3;
    for i in 0..n { n = 5; print_i32(i); }
    print_i32(n);
    return 0;
}
"##, r##"0
1
2
5
"##),
    ("match_array_scrutinee_binding", r##"fn main() -> i32 {
    let a = [1, 2];
    match a { b => print_i32(b[1]) }
    return 0;
}
"##, r##"2
"##),
    ("match_struct_binding", r##"struct P { x: i32 }
fn main() -> i32 {
    let p = P { x: 3 };
    let r = match p { q => q.x + 1 };
    print_i32(r);
    return 0;
}
"##, r##"4
"##),
    ("match_unit_pattern", r##"fn u() { }
fn main() -> i32 {
    match u() { () => println("unit") }
    return 0;
}
"##, r##"unit
"##),
    ("tuple_return_match", r##"fn f() -> (i32, bool) { return (7, true); }
fn main() -> i32 {
    let r = match f() { (7, false) => 0, (7, true) => 1, (_, _) => 2 };
    print_i32(r);
    return 0;
}
"##, r##"1
"##),
    ("arm_comma_rules", r##"fn main() -> i32 {
    let a = match 1 { 1 => 10, 2 => 20, _ => 30, };
    let b = match 2 { 1 => { 10 }, 2 => { 20 }, _ => { 30 }, };
    let c = match 3 { 1 => { 10 } 2 => 20, _ => { 30 } };
    print_i32(a + b + c);
    return 0;
}
"##, r##"60
"##),
    ("scrut_mutated_in_arm", r##"fn main() -> i32 {
    let mut t = (1, 2);
    match t { (a, b) => { t.0 = 50; print_i32(a); print_i32(t.0); print_i32(b); } }
    let mut arr = [1, 2];
    match arr { b => { arr[0] = 9; print_i32(b[0]); print_i32(arr[0]); } }
    return 0;
}
"##, r##"1
50
2
1
9
"##),
    ("payload_agg_alias", r##"struct P { x: i32 }
enum E { W(P), N }
fn main() -> i32 {
    let mut e = E::W(P { x: 1 });
    match e {
        E::W(p) => { e = E::N; print_i32(p.x); }
        E::N => { }
    }
    let mut s = P { x: 5 };
    match (s, 1) { (q, _) => { s.x = 9; print_i32(q.x); print_i32(s.x); } }
    return 0;
}
"##, r##"1
5
9
"##),
    ("nested_tuple_payload_mut", r##"enum E { T((i32, i32)), U }
fn main() -> i32 {
    let mut e = E::T((1, 2));
    match e { E::T((a, b)) => { e = E::U; print_i32(a * 10 + b); } E::U => {} }
    return 0;
}
"##, r##"12
"##),
    ("value_match_assign_inside", r##"fn main() -> i32 {
    let mut count = 0;
    let mut i = 0;
    while i < 5 {
        let r = match i % 2 { 0 => { count += 1; 10 } _ => { count += 100; 20 } };
        print_i32(r + count);
        i += 1;
    }
    return 0;
}
"##, r##"11
121
112
222
213
"##),
    ("match_expr_as_array_index_and_elem", r##"fn main() -> i32 {
    let a = [10, 20, 30];
    let i = 1;
    print_i32(a[match i { 1 => 2, _ => 0 }]);
    let b = [match i { 1 => 7, _ => 8 }, 3];
    print_i32(b[0] + b[1]);
    let t = (match i { 1 => 7, _ => 8 }, match i { 0 => 1, _ => 2 });
    print_i32(t.0 * 10 + t.1);
    return 0;
}
"##, r##"30
10
72
"##),
    ("match_in_struct_lit", r##"struct P { x: i32, y: i32 }
fn main() -> i32 {
    let p = P { x: match 1 { 1 => 4, _ => 0 }, y: match 2 { 1 => 0, _ => 9 } };
    print_i32(p.x + p.y);
    return 0;
}
"##, r##"13
"##),
    ("match_unary_cast", r##"fn main() -> i32 {
    let a = -match 3 { 3 => 5, _ => 0 };
    let b = match 1 { 1 => 7, _ => 0 } as i64;
    print_i32(a); print_i64(b);
    print_bool(!match 1 { 1 => false, _ => true });
    return 0;
}
"##, r##"-5
7
true
"##),
    ("match_returns_agg", r##"enum E { A(i32), B }
fn mk(n: i32) -> E { match n { 0 => E::B, k => E::A(k) } }
fn main() -> i32 {
    match mk(0) { E::B => println("B"), E::A(_) => println("A") }
    match mk(4) { E::B => println("B"), E::A(x) => print_i32(x) }
    let t = match 1 { 1 => (1, 2), _ => (3, 4) };
    print_i32(t.1);
    let a = match 1 { 1 => [1, 2, 3], _ => [4, 5, 6] };
    print_i32(a[2]);
    return 0;
}
"##, r##"B
4
2
3
"##),
    ("match_returns_string_concat", r##"fn main() -> i32 {
    let s = "a" + match 1 { 1 => "b", _ => "c" } + "d";
    println(s);
    return 0;
}
"##, r##"abd
"##),
    ("match_inside_match_arm_stmt", r##"fn main() -> i32 {
    let x = 1;
    match x {
        1 => {
            match 2 { 2 => { println("inner"); } _ => {} }
            println("after");
        }
        _ => {}
    }
    return 0;
}
"##, r##"inner
after
"##),
    ("match_nested_tail_value", r##"fn pick(a: i32, b: i32) -> i32 {
    match a {
        0 => match b { 0 => 1, _ => 2 },
        _ => { match b { 0 => 3, _ => 4 } }
    }
}
fn main() -> i32 { print_i32(pick(0,0)); print_i32(pick(0,1)); print_i32(pick(1,0)); print_i32(pick(1,1)); return 0; }
"##, r##"1
2
3
4
"##),
    ("loop_with_match_tail_stmt", r##"fn main() -> i32 {
    let mut i = 0;
    while i < 3 {
        match i { 1 => { println("one"); } _ => { print_i32(i); } }
        i += 1;
    }
    for j in 0..2 { match j { 0 => { continue; } _ => { println("j1"); } } }
    return 0;
}
"##, r##"0
one
2
j1
"##),
    ("match_in_unit_fn_arms_with_values_ok", r##"fn f(x: i32) { match x { 1 => 5, _ => 6 } }
fn main() -> i32 { f(1); println("ok"); return 0; }
"##, r##"ok
"##),
    ("bool_tuple_exhaustive", r##"fn f(a: bool, b: bool) -> i32 {
    match (a, b) { (true, true) => 3, (true, false) => 2, (false, true) => 1, (false, false) => 0 }
}
fn main() -> i32 { print_i32(f(true,true)); print_i32(f(false,true)); print_i32(f(false,false)); return 0; }
"##, r##"3
1
0
"##),
    ("i64_literal_patterns", r##"fn f(x: i64) -> i32 { match x { 0 => 0, 5000000000 => 1, -1 => 2, _ => 3 } }
fn main() -> i32 { print_i32(f(5000000000)); print_i32(f(-1)); print_i32(f(7)); return 0; }
"##, r##"1
2
3
"##),
    ("wildcard_then_arm_warn_but_runs", r##"fn main() -> i32 {
    let r = match 3 { x => x, 3 => 0 };
    print_i32(r);
    return 0;
}
"##, r##"3
"##),
    ("match_diverge_via_if_both", r##"fn f(n: i32) -> i32 {
    let v = match n { 0 => { if n == 0 { return 1; } else { return 2; } } k => k };
    return v + 100;
}
fn main() -> i32 { print_i32(f(0)); print_i32(f(5)); return 0; }
"##, r##"1
105
"##),
    ("arm_return_no_value_in_unit", r##"fn f(x: i32) {
    let y = match x { 0 => return, k => k };
    print_i32(y);
}
fn main() -> i32 { f(0); f(3); return 0; }
"##, r##"3
"##),
    ("match_expr_bare_return_arm_value_fn", r##"fn f(x: i32) -> i32 {
    let y = match x { 0 => return 7, k => k * 2 };
    return y;
}
fn main() -> i32 { print_i32(f(0)); print_i32(f(4)); return 0; }
"##, r##"7
8
"##),
    ("match_in_cond", r##"fn main() -> i32 {
    if match 2 { 2 => true, _ => false } { println("yes"); }
    let mut n = 0;
    while match n { 3 => false, _ => true } { n += 1; }
    print_i32(n);
    return 0;
}
"##, r##"yes
3
"##),
    ("tuple_of_tuple_match_wild", r##"fn main() -> i32 {
    let t = ((1, 2), (3, 4));
    let r = match t { ((_, b), (c, _)) => b * 10 + c };
    print_i32(r);
    return 0;
}
"##, r##"23
"##),
];

const REJECT: &[(&str, &str, &str)] = &[
    ("missing_deep_example", r##"enum E { P((i32, bool), i32), Q }
fn f(e: E) -> i32 { match e { E::P((_, true), _) => 1, E::Q => 2 } }
fn main() -> i32 { return f(E::Q); }
"##, r##"E::P((_, false), _)"##),
    ("duplicate_deep_arm_error", r##"enum E { P(i32, bool), Q }
fn main() -> i32 {
    match E::Q { E::P(1, true) => { } E::P(1, true) => { } _ => { } }
    return 0;
}
"##, r##"E0271"##),
    ("missing_literal_combo", r##"enum E { A(i32), B }
fn g(e: E) -> i32 { match e { E::A(1) => 10, E::B => 30 } }
fn main() -> i32 { return g(E::B); }
"##, r##"E0270"##),
    ("missing_msg_example", r##"enum E { A(i32), B }
fn g(e: E) -> i32 { match e { E::A(1) => 10, E::A(2) => 1, E::B => 30 } }
fn main() -> i32 { return g(E::B); }
"##, r##"E::A(_)"##),
    ("tuple_nonexhaustive", r##"fn main() -> i32 {
    let t = (1, true);
    match t { (_, true) => { } }
    return 0;
}
"##, r##"(_, false)"##),
    ("bool_match_missing", r##"fn f(b: bool) -> i32 { match b { true => 1 } }
fn main() -> i32 { return f(true); }
"##, r##"E0270"##),
    ("match_expr_types_mismatch", r##"fn main() -> i32 {
    let n = 5;
    let r = match n { 0 => 1, _ => "s" };
    return 0;
}
"##, r##"E0273"##),
    ("match_expr_unit_arm_mismatch", r##"fn main() -> i32 {
    let n = 5;
    let r = match n { 0 => 1, _ => { } };
    return 0;
}
"##, r##"E0273"##),
    ("dup_variant_arm", r##"enum E { A, B }
fn main() -> i32 {
    match E::A { E::A => { } E::A => { } E::B => { } }
    return 0;
}
"##, r##"E0271"##),
    ("binding_dup_in_pattern", r##"fn main() -> i32 {
    let t = (1, 2);
    match t { (a, a) => { } }
    return 0;
}
"##, r##"E0274"##),
    ("float_pattern_rejected", r##"enum E { A(f64) }
fn main() -> i32 { match E::A(1.0) { E::A(1.0) => { } _ => { } } return 0; }
"##, r##"E0268"##),
    ("missing_nested", r##"enum In { A, B }
enum Out { W(In), N }
fn f(o: Out) -> i32 { match o { Out::W(In::A) => 1, Out::N => 2 } }
fn main() -> i32 { return f(Out::N); }
"##, r##"Out::W(In::B)"##),
    ("literal_0x80000000_i32", r##"fn main() -> i32 { let a = 0x80000000; return 0; }
"##, r##"E0263"##),
    ("literal_bad_prefix", r##"fn main() -> i32 { let a = 0b; return 0; }
"##, r##"E0"##),
    ("literal_bad_octal", r##"fn main() -> i32 { let a = 0o8; return 0; }
"##, r##"E0"##),
    ("unicode_bad_escape", r##"fn main() -> i32 { let s = "\u{110000}"; return 0; }
"##, r##"E0005"##),
    ("missing_return_nonexhaustive_all_return", r##"enum E { A, B }
fn f(e: E) -> i32 { match e { E::A => { return 1; } } }
fn main() -> i32 { return f(E::A); }
"##, r##"E0270"##),
    ("missing_return_some_arm", r##"enum E { A, B }
fn f(e: E) -> i32 { match e { E::A => { return 1; } E::B => { } } }
fn main() -> i32 { return f(E::A); }
"##, r##"E02"##),
    ("arm_missing_comma", r##"fn main() -> i32 {
    let a = match 1 { 1 => 10 2 => 20, _ => 30 };
    return 0;
}
"##, r##"E0"##),
    ("match_noarms", r##"fn main() -> i32 { let x = 1; match x { } return 0; }
"##, r##"E0270"##),
    ("binding_immutable", r##"fn main() -> i32 { match 1 { x => { x = 2; } } return 0; }
"##, r##"E0"##),
];

const WARN: &[(&str, &str, &str)] = &[
    ("unreachable_deep_arm_warning", r##"enum E { P(i32, bool), Q }
fn main() -> i32 {
    match E::Q { E::P(_, _) => { } E::P(1, true) => { } E::Q => { } }
    return 0;
}
"##, r##"W0272"##),
    ("unreachable_warn", r##"fn main() -> i32 {
    let x = 1;
    match x { _ => { println("a"); } 1 => { println("b"); } }
    return 0;
}
"##, r##"W0272"##),
    ("char_string_unreachable_dup", r##"fn main() -> i32 {
    match 'a' { 'a' => {} 'a' => {} _ => {} }
    return 0;
}
"##, r##"W0272"##),
];

const RUNTIME: &[(&str, &str, &str)] = &[
    ("div_zero_rt", r##"fn main() -> i32 { let z = 0; print_i32(5 / z); return 0; }
"##, r##"division by zero"##),
];

#[test]
fn programs_agree_at_o0_and_o2_with_hand_derived_output() {
    for (name, src, want) in RUN {
        for level in [0u8, 2] {
            let got = run_source("audit.ae", src, level)
                .unwrap_or_else(|e| panic!("{name} -O{level} failed:\n{e}\n{src}"));
            assert_eq!(&got.1, want, "{name} -O{level}\n{src}");
        }
    }
}

#[test]
fn invalid_programs_are_rejected_with_the_expected_diagnostic() {
    for (name, src, code) in REJECT {
        let (errors, rendered) = compile_diags(src);
        assert!(
            errors && rendered.contains(code),
            "{name}: expected an error containing `{code}`, got:\n{rendered}\n{src}"
        );
        assert!(!rendered.contains("panicked"), "{name}");
    }
}

#[test]
fn unreachable_arms_warn_but_compile() {
    for (name, src, code) in WARN {
        let (errors, rendered) = compile_diags(src);
        assert!(
            !errors && rendered.contains(code),
            "{name}: expected a warning containing `{code}`, got:\n{rendered}\n{src}"
        );
    }
}

#[test]
fn runtime_errors_surface_on_the_vm() {
    for (name, src, msg) in RUNTIME {
        for level in [0u8, 2] {
            match run_source("audit.ae", src, level) {
                Err(e) => assert!(e.contains(msg), "{name} -O{level}: {e}"),
                Ok(_) => panic!("{name} -O{level}: expected runtime error `{msg}`"),
            }
        }
    }
}

/// Deepest nest of `match` expressions that the parser's limit still allows
/// must compile and run on a default 2 MiB test thread (debug build).
#[test]
fn deeply_nested_match_expressions_and_blocks_stay_within_the_limit() {
    fn nest(depth: usize) -> String {
        let mut s = String::from("0");
        for i in 0..depth {
            s = format!("match {i} {{ _ => {s} }}");
        }
        format!("fn main() -> i32 {{ let r = {s}; print_i32(r); return 0; }}")
    }
    let mut deepest_ok = 0;
    for depth in [10, 40, 80, 100, 120, 126, 127, 128, 200, 300] {
        let src = nest(depth);
        let (errors, rendered) = compile_diags(&src);
        if errors {
            assert!(rendered.contains("E0101"), "depth {depth}: {rendered}");
        } else {
            deepest_ok = depth;
            let out = run_source("deep.ae", &src, 2).expect("runs");
            assert_eq!(out.1, "0\n", "depth {depth}");
        }
    }
    assert!(deepest_ok >= 100, "limit too tight: {deepest_ok}");

    // nested patterns hit the same limit with a diagnostic, not a crash
    let mut pat = String::from("x");
    let mut val = String::from("1");
    for _ in 0..300 {
        pat = format!("({pat}, _)");
        val = format!("({val}, 0)");
    }
    let src = format!("fn main() -> i32 {{ match {val} {{ {pat} => {{ }} }} return 0; }}");
    let (errors, rendered) = compile_diags(&src);
    assert!(errors && rendered.contains("E0101"), "{rendered}");
}

#[test]
fn lexical_audit_findings() {
    // unknown escapes and empty char literals are errors, not silently kept
    for (src, code) in [
        (r#"fn main() -> i32 { let s = "a\qb"; return 0; }"#, "E0006"),
        (r#"fn main() -> i32 { let s = "\u41"; return 0; }"#, "E0006"),
        ("fn main() -> i32 { let c = ''; return 0; }", "E0003"),
    ] {
        let (errors, rendered) = compile_diags(src);
        assert!(errors && rendered.contains(code), "{src}\n{rendered}");
    }
    // a UTF-8 byte order mark is skipped
    let src = "\u{feff}fn main() -> i32 { print_i32(7); return 0; }";
    assert_eq!(run_source("bom.ae", src, 0).expect("runs").1, "7\n");
    // `for` bounds are evaluated once, even when the body rewrites the variable
    let src = "fn main() -> i32 { let mut n = 3; for i in 0..n { n = 5; print_i32(i); } print_i32(n); return 0; }";
    for level in [0u8, 2] {
        assert_eq!(run_source("for.ae", src, level).expect("runs").1, "0\n1\n2\n5\n");
    }
}

/// Liveness used to stop after 64 rounds, so on a deeply nested aggregate
/// comparison DCE dropped a value that was still live (-O1/-O2 printed `()`).
#[test]
fn deeply_nested_tuple_equality_survives_optimisation() {
    for depth in [67usize] {
        let (mut ty, mut val) = (String::from("i32"), String::from("1"));
        for _ in 0..depth {
            ty = format!("({ty}, i32)");
            val = format!("({val}, 2)");
        }
        let chain = ".0".repeat(depth);
        let pat = (0..depth).fold(String::from("x"), |p, _| format!("({p}, _)"));
        let src = format!(
            "fn main() -> i32 {{ let t: {ty} = {val}; let u = t; print_bool(u == t); \
             let r = u{chain}; print_i32(r); let m = match u {{ {pat} => x }}; print_i32(m); return 0; }}"
        );
        for level in [0u8, 1, 2] {
            let out = run_source("deep.ae", &src, level)
                .unwrap_or_else(|e| panic!("depth {depth} -O{level}: {e}"));
            assert_eq!(out.1, "true\n1\n1\n", "depth {depth} -O{level}");
        }
    }
}

/// DCE used to delete unused `/`, `%` and indexing, so -O1/-O2 skipped a
/// run-time error that -O0 reported.
#[test]
fn dead_trapping_operations_still_trap_when_optimised() {
    for (src, msg) in [
        ("fn main() -> i32 { let z = 0; let q = 5 / z; return 0; }", "division by zero"),
        ("fn main() -> i32 { let z = 0; let q = 5 % z; return 0; }", "division by zero"),
        ("fn main() -> i32 { let z: i64 = 0; let q = 5 / z; return 0; }", "division by zero"),
        ("fn main() -> i32 { let a = [1, 2]; let i = 5; let x = a[i]; return 0; }", "out of bounds"),
        ("fn main() -> i32 { let s = \"ab\"; let i = 5; let x = s[i]; return 0; }", "out of bounds"),
    ] {
        for level in [0u8, 1, 2] {
            match run_source("trap.ae", src, level) {
                Err(e) => assert!(e.contains(msg), "-O{level} {src}: {e}"),
                Ok(_) => panic!("-O{level}: `{src}` should fail with `{msg}`"),
            }
        }
    }
    // float division never traps and may still be dropped
    let ok = "fn main() -> i32 { let z = 0.0; let q = 5.0 / z; return 0; }";
    for level in [0u8, 2] {
        assert!(run_source("trap.ae", ok, level).is_ok());
    }
}

#[test]
fn fmt_is_a_fixpoint_for_an_arm_block_holding_one_match() {
    use std::process::Command;
    let dir = std::env::temp_dir().join(format!("aether_fmtfix_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = "enum E { A(i32), B }\nfn main() -> i32 {\n let e = E::A(1);\n match e {\n E::A(x) => { match x { 1 => { print_i32(1); } _ => { print_i32(2); } } }\n E::B => { print_i32(3); }\n }\n return 0;\n}\n";
    let f = dir.join("a.ae");
    std::fs::write(&f, src).unwrap();
    let fmt = |p: &std::path::Path| {
        let o = Command::new(env!("CARGO_BIN_EXE_aether")).args(["fmt", p.to_str().unwrap()]).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    let one = fmt(&f);
    let g = dir.join("b.ae");
    std::fs::write(&g, &one).unwrap();
    assert_eq!(one, fmt(&g), "fmt(fmt(p)) != fmt(p)");
    let _ = std::fs::remove_dir_all(&dir);
}

//! Enums with payloads, `match` / `if let`, and tuples: every program runs
//! at -O0 and -O2 on the VM and both must agree on exit value and output.

use aether::{compile_source, run_source, CompileOptions, Value};

fn run(src: &str, level: u8) -> (Value, String) {
    run_source("enums.ae", src, level)
        .map(|(v, out, _)| (v, out))
        .unwrap_or_else(|e| panic!("-O{level} failed:\n{e}\n{src}"))
}

/// Runs at -O0 and -O2 and asserts the exit value and stdout.
fn check(src: &str, value: i32, stdout: &str) {
    let o0 = run(src, 0);
    let o2 = run(src, 2);
    assert_eq!(o0, o2, "-O0 and -O2 disagree\n{src}");
    assert_eq!(o0.0, Value::I32(value), "{src}");
    assert_eq!(o0.1, stdout, "{src}");
}

/// Asserts that compilation fails with the given error code.
fn rejects(src: &str, code: &str) {
    let opts = CompileOptions {
        opt_level: 0,
        color: false,
    };
    let c = compile_source("enums.ae", src, &opts);
    let rendered = c.diags.render(&c.session, false);
    assert!(
        c.diags.has_errors() && rendered.contains(code),
        "expected {code}, got:\n{rendered}\n{src}"
    );
}

const SHAPE: &str = "
enum Shape {
    Circle(f64),
    Rect(i32, i32),
    Empty,
}
";

#[test]
fn tuple_create_access_destructure_equality() {
    check(
        "fn main() -> i32 {
            let t = (1, true, \"s\");
            let (a, b, c) = t;
            print_i32(t.0);
            print_bool(t.1);
            println(c);
            print_bool(b);
            let u: (i32, i64) = (3, 4);
            let v = (3, 4 as i64);
            print_bool(u == v);
            print_bool(u != v);
            print_bool((1, 2) == (1, 3));
            print_bool(((1, 2), 3) == ((1, 2), 3));
            return a + u.0 + (u.1 as i32);
        }",
        8,
        "1\ntrue\ns\ntrue\ntrue\nfalse\nfalse\ntrue\n",
    );
}

#[test]
fn tuple_fields_are_assignable_and_nested_index_parses() {
    check(
        "fn main() -> i32 {
            let mut t = (1, (2, 3));
            t.0 = 10;
            t.1.1 = 30;
            let (x, y) = t.1;
            return t.0 + x + y;
        }",
        42,
        "",
    );
}

#[test]
fn enum_payloads_and_match_every_variant() {
    check(
        &format!(
            "{SHAPE}
            fn area(s: Shape) -> i32 {{
                match s {{
                    Shape::Circle(r) => {{ return (3.0 * r * r) as i32; }}
                    Shape::Rect(w, h) => {{ return w * h; }}
                    Shape::Empty => {{ return 0; }}
                }}
            }}
            fn main() -> i32 {{
                let c = Shape::Circle(2.0);
                let r = Shape::Rect(3, 4);
                let e = Shape::Empty;
                print_i32(area(c));
                print_i32(area(r));
                print_i32(area(e));
                return area(c) + area(r) + area(e);
            }}"
        ),
        24,
        "12\n12\n0\n",
    );
}

#[test]
fn match_with_wildcard_and_binding_arms() {
    check(
        &format!(
            "{SHAPE}
            fn kind(s: Shape) -> i32 {{
                match s {{
                    Shape::Rect(_, h) => {{ return h; }}
                    _ => {{ return -1; }}
                }}
            }}
            fn main() -> i32 {{
                let s = Shape::Circle(1.0);
                let mut n = 0;
                match s {{
                    other => {{
                        n = kind(other) + kind(Shape::Rect(1, 5));
                    }}
                }}
                return n;
            }}"
        ),
        4,
        "",
    );
}

#[test]
fn match_on_scalars_with_literals() {
    check(
        "fn digit(c: char) -> i32 {
            match c {
                '0' => { return 0; }
                '1' => { return 1; }
                _ => { return -1; }
            }
        }
        fn word(s: string) -> i32 {
            match s {
                \"one\" => { return 1; }
                \"two\" => { return 2; }
                _ => { return 0; }
            }
        }
        fn main() -> i32 {
            let mut acc = 0;
            for i in -1..3 {
                match i {
                    -1 => { acc = acc + 100; }
                    0 => { acc = acc + 1; }
                    1 => { acc = acc + 10; }
                    _ => { acc = acc + 1000; }
                }
            }
            let flag = true;
            match flag {
                true => { print(\"t\"); }
                _ => { print(\"f\"); }
            }
            let big: i64 = 5000000000;
            match big {
                5000000000 => { print(\"big\"); }
                _ => { print(\"small\"); }
            }
            return acc + digit('1') + word(\"two\") * 10 + word(\"x\");
        }",
        1132,
        "tbig",
    );
}

#[test]
fn if_let_with_and_without_else() {
    check(
        &format!(
            "{SHAPE}
            fn main() -> i32 {{
                let s = Shape::Rect(6, 7);
                let mut n = 0;
                if let Shape::Rect(w, h) = s {{
                    n = w * h;
                }} else {{
                    n = -1;
                }}
                if let Shape::Circle(r) = s {{
                    n = r as i32;
                }}
                if let Shape::Empty = Shape::Empty {{
                    n = n + 1;
                }} else if n == 0 {{
                    n = 99;
                }}
                return n;
            }}"
        ),
        43,
        "",
    );
}

#[test]
fn enum_in_struct_struct_in_enum_payload_and_enum_in_array() {
    check(
        "struct Point { x: i32, y: i32 }
        enum Event { Click(Point), Key(char), Quit }
        struct Window { title: string, last: Event }
        fn main() -> i32 {
            let w = Window { title: \"w\", last: Event::Click(Point { x: 3, y: 4 }) };
            let events = [Event::Key('a'), w.last, Event::Quit];
            let mut sum = 0;
            for i in 0..3 {
                match events[i] {
                    Event::Click(p) => { sum = sum + p.x * p.y; }
                    Event::Key(c) => { print_char(c); }
                    Event::Quit => { sum = sum + 100; }
                }
            }
            print_bool(events[1] == w.last);
            print_bool(events[0] == Event::Key('b'));
            print_bool(Event::Quit == Event::Quit);
            print_bool(Event::Quit != Event::Key('a'));
            return sum;
        }",
        112,
        "a\ntrue\nfalse\ntrue\ntrue\n",
    );
}

#[test]
fn match_arm_with_return_break_and_continue() {
    check(
        &format!(
            "{SHAPE}
            fn first_rect(xs: [Shape; 4]) -> i32 {{
                let mut i = 0;
                while i < 4 {{
                    match xs[i] {{
                        Shape::Rect(w, _) => {{ return w; }}
                        _ => {{ }}
                    }}
                    i = i + 1;
                }}
                return -1;
            }}
            fn main() -> i32 {{
                let xs = [Shape::Empty, Shape::Circle(1.0), Shape::Rect(8, 1), Shape::Rect(9, 9)];
                let mut seen = 0;
                for i in 0..4 {{
                    match xs[i] {{
                        Shape::Empty => {{ continue; }}
                        Shape::Rect(_, _) => {{
                            if seen > 1 {{ break; }}
                            seen = seen + 1;
                        }}
                        _ => {{ seen = seen + 1; }}
                    }}
                }}
                print_i32(seen);
                return first_rect(xs);
            }}"
        ),
        8,
        "2\n",
    );
}

#[test]
fn enums_and_tuples_have_value_semantics_across_calls() {
    check(
        &format!(
            "{SHAPE}
            fn mutate(mut_t: (i32, i32)) -> (i32, i32) {{
                let mut t = mut_t;
                t.0 = 100;
                return t;
            }}
            fn swap(s: Shape) -> Shape {{
                match s {{
                    Shape::Rect(w, h) => {{ return Shape::Rect(h, w); }}
                    _ => {{ return s; }}
                }}
            }}
            fn main() -> i32 {{
                let t = (1, 2);
                let u = mutate(t);
                print_i32(t.0);
                print_i32(u.0);
                let r = Shape::Rect(1, 2);
                let s = swap(r);
                let mut n = 0;
                if let Shape::Rect(w, _) = r {{ n = n + w; }}
                if let Shape::Rect(w, _) = s {{ n = n + w * 10; }}
                let mut arr = [r, r];
                arr[0] = Shape::Empty;
                print_bool(arr[1] == r);
                return n;
            }}"
        ),
        21,
        "1\n100\ntrue\n",
    );
}

#[test]
fn match_without_catch_all_on_enum_is_exhaustive_when_all_variants_covered() {
    check(
        "enum Tri { A, B, C }
        fn main() -> i32 {
            let x = Tri::C;
            match x {
                Tri::A => { return 1; }
                Tri::B => { return 2; }
                Tri::C => { return 3; }
            }
        }",
        3,
        "",
    );
}

#[test]
fn exhaustiveness_and_duplicate_arm_errors() {
    rejects(
        &format!(
            "{SHAPE}
            fn main() -> i32 {{
                match Shape::Empty {{
                    Shape::Circle(r) => {{ }}
                    Shape::Empty => {{ }}
                }}
                return 0;
            }}"
        ),
        "E0270",
    );
    rejects(
        "fn main() -> i32 { let x = 3; match x { 1 => { } 2 => { } } return 0; }",
        "E0270",
    );
    rejects(
        &format!(
            "{SHAPE}
            fn main() -> i32 {{
                match Shape::Empty {{
                    Shape::Empty => {{ }}
                    Shape::Empty => {{ }}
                    _ => {{ }}
                }}
                return 0;
            }}"
        ),
        "E0271",
    );
}

#[test]
fn enum_and_tuple_type_errors() {
    rejects(&format!("{SHAPE} fn main() -> i32 {{ let s = Shape::Square(1); return 0; }}"), "E0266");
    rejects(&format!("{SHAPE} fn main() -> i32 {{ let s = Shape::Rect(1); return 0; }}"), "E0267");
    rejects(&format!("{SHAPE} fn main() -> i32 {{ let s = Shape::Circle(1); return 0; }}"), "E0269");
    rejects("fn main() -> i32 { let s = Nope::X; return 0; }", "E0265");
    rejects(&format!("{SHAPE} fn main() -> i32 {{ let b = Shape::Empty < Shape::Empty; return 0; }}"), "E0244");
    rejects("fn main() -> i32 { let t = (1, 2); let (a, b, c) = t; return 0; }", "E0269");
    rejects("fn main() -> i32 { let t = (1, 2); return t.2; }", "E0248");
    rejects("fn main() -> i32 { let t = (1, 2); match t { 1 => { } _ => { } } return 0; }", "E0269");
    rejects("enum E { A, A } fn main() -> i32 { return 0; }", "E0204");
    rejects("struct S { x: i32 } enum S { A } fn main() -> i32 { return 0; }", "E0201");
    rejects("enum E { A(E) } fn main() -> i32 { return 0; }", "E0205");
    rejects(
        &format!("{SHAPE} fn main() -> i32 {{ match Shape::Empty {{ Shape::Circle(1.5) => {{ }} _ => {{ }} }} return 0; }}"),
        "E0268",
    );
}

#[test]
fn match_is_a_return_path_and_prints_through_functions() {
    check(
        &format!(
            "{SHAPE}
            fn name(s: Shape) -> string {{
                match s {{
                    Shape::Circle(_) => {{ return \"circle\"; }}
                    Shape::Rect(w, h) => {{
                        if w == h {{ return \"square\"; }}
                        return \"rect\";
                    }}
                    Shape::Empty => {{ return \"empty\"; }}
                }}
            }}
            fn pair() -> (string, Shape) {{ return (\"p\", Shape::Rect(2, 2)); }}
            fn main() -> i32 {{
                let (label, s) = pair();
                println(label + name(s));
                println(name(Shape::Circle(0.5)) + name(Shape::Empty));
                return 0;
            }}"
        ),
        0,
        "psquare\ncircleempty\n",
    );
}

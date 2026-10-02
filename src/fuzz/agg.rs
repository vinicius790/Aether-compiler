//! Aggregate-heavy program generator.
//!
//! `gen_program` (sketch.rs) only knows `i32` and `bool`. This generator
//! produces well-typed, always-terminating, runtime-error-free programs
//! that lean on everything it never touched: structs (nested, with literal
//! fields written in random order), fixed and nested arrays, element /
//! field / nested assignment, `for` and `while`, `&&` / `||` guards whose
//! right side is only safe thanks to short-circuit, `i64` / `f64` / `char`
//! / `string` arithmetic and comparison, casts, tail-expression bodies,
//! negative literals, a `for` variable shadowing an outer binding, and
//! helpers that mutate a by-value copy of a struct or array while the
//! caller prints the original afterwards.
//!
//! Every value the program computes is printed, so captured stdout is the
//! differential oracle. Integer magnitudes are tracked per binding so they
//! stay below 1000, indices outside guards are literal and in bounds, and
//! there is no division.

use super::rng::FuzzRng;

const LIMIT: i64 = 1000;
const WORDS: &[&str] = &["ab", "cd", "xyz", "hello", "aether", ""];
const LETTERS: &[char] = &['a', 'b', 'c', 'k', 'q', 'z'];
const CMP: &[&str] = &["<", "<=", ">", ">=", "==", "!="];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Int {
    I32,
    I64,
}

pub fn gen_aggregate_program(rng: &mut FuzzRng) -> String {
    let mut g = Gen {
        rng,
        out: String::new(),
        indent: 0,
        bounds: Vec::new(),
        arr_len: 0,
        loop_var: None,
        fresh: 0,
    };
    g.program();
    g.out
}

struct Gen<'r> {
    rng: &'r mut FuzzRng,
    out: String,
    indent: usize,
    /// Magnitude bound of every numeric binding keyed by its spelling
    /// (`s`, `a`, `r.inner.x`, ...). Arrays share one bound per array.
    bounds: Vec<(String, i64)>,
    arr_len: usize,
    loop_var: Option<String>,
    fresh: u32,
}

impl<'r> Gen<'r> {
    // ----- emission helpers -----

    fn line(&mut self, s: impl AsRef<str>) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
        self.out.push_str(s.as_ref());
        self.out.push('\n');
    }

    fn open(&mut self, s: impl AsRef<str>) {
        self.line(s);
        self.indent += 1;
    }

    fn else_branch(&mut self) {
        self.indent -= 1;
        self.line("} else {");
        self.indent += 1;
    }

    fn close(&mut self) {
        self.indent -= 1;
        self.line("}");
    }

    fn bound(&self, key: &str) -> i64 {
        self.bounds
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, b)| *b)
            .unwrap_or(LIMIT)
    }

    fn set_bound(&mut self, key: &str, b: i64) {
        if let Some(e) = self.bounds.iter_mut().find(|(k, _)| k == key) {
            e.1 = b;
        } else {
            self.bounds.push((key.to_string(), b));
        }
    }

    fn lit(&mut self) -> i64 {
        self.rng.int(0, 9) as i64
    }

    fn signed_lit(&mut self) -> String {
        let v = self.lit();
        if self.rng.bool() {
            format!("-{v}")
        } else {
            v.to_string()
        }
    }

    fn float_lit(&mut self) -> (String, i64) {
        let w = self.rng.int(0, 9);
        let fr = self.rng.int(0, 9);
        (format!("{w}.{fr}"), w as i64 + 1)
    }

    fn idx(&mut self) -> usize {
        self.rng.int(0, self.arr_len as i32 - 1) as usize
    }

    fn fresh(&mut self, prefix: &str) -> String {
        self.fresh += 1;
        format!("{prefix}{}", self.fresh)
    }

    fn word(&mut self) -> &'static str {
        // explicit deref: rustc 1.75 otherwise infers `T = str` from the return type
        *self.rng.choose(WORDS)
    }

    fn letter(&mut self) -> char {
        *self.rng.choose(LETTERS)
    }

    fn shuffle(&mut self, mut v: Vec<String>) -> Vec<String> {
        for i in (1..v.len()).rev() {
            let j = self.rng.int(0, i as i32) as usize;
            v.swap(i, j);
        }
        v
    }

    // ----- program skeleton -----

    fn program(&mut self) {
        self.arr_len = self.rng.int(2, 4) as usize;
        self.line("struct Inner { x: i32, y: i32 }");
        self.line("struct Rec { a: i32, b: i64, c: f64, d: bool, inner: Inner }");
        self.line("");
        self.helpers();
        self.main();
    }

    fn helpers(&mut self) {
        let k = self.lit();
        if self.rng.bool() {
            self.line(format!("fn bump(x: i32) -> i32 {{ x + {k} }}"));
        } else {
            self.line(format!("fn bump(x: i32) -> i32 {{ return x + {k}; }}"));
        }
        if self.rng.bool() {
            self.line("fn twice(x: i64) -> i64 { x * 2 }");
        } else {
            self.line("fn twice(x: i64) -> i64 { return x + x; }");
        }
        // Struct by value: the callee mutates its copy (nested field too),
        // the caller prints the original afterwards.
        self.open("fn poke(p: Rec) -> i32 {");
        self.line("let mut c = p;");
        let a = self.lit();
        let x = self.lit();
        self.line(format!("c.a = {a};"));
        self.line(format!("c.inner.x = {x};"));
        self.line("c.b = -(c.b);");
        self.line("print_i32(c.a);");
        self.line("print_i32(c.inner.x);");
        self.line("print_i64(c.b);");
        self.line("print_i32(p.a);");
        if self.rng.bool() {
            self.line("c.a + c.inner.x");
        } else {
            self.line("return c.a + c.inner.x;");
        }
        self.close();
        let n = self.arr_len;
        self.open(format!("fn fill(a: [i32; {n}]) -> i32 {{"));
        self.line("let mut c = a;");
        let v = self.lit();
        self.line(format!("c[0] = {v};"));
        self.line("print_i32(c[0]);");
        self.line("print_i32(a[0]);");
        self.line(format!("return c[0] + c[{}];", n - 1));
        self.close();
        self.open("fn flip(g: [[i32; 2]; 2]) -> i32 {");
        self.line("let mut c = g;");
        let v = self.lit();
        self.line(format!("c[1][0] = {v};"));
        self.line("print_i32(c[1][0]);");
        self.line("c[0][0] + c[1][1]");
        self.close();
        self.line("");
    }

    fn main(&mut self) {
        self.open("fn main() -> i32 {");
        // struct literal with fields in random order
        let a0 = self.lit();
        let b0 = self.signed_lit();
        let (c0, cb) = self.float_lit();
        let d0 = self.rng.bool();
        let x0 = self.lit();
        let y0 = self.lit();
        let inner = self.shuffle(vec![format!("x: {x0}"), format!("y: {y0}")]);
        let inner = format!("Inner {{ {} }}", inner.join(", "));
        let fields = self.shuffle(vec![
            format!("a: {a0}"),
            format!("b: {b0}"),
            format!("c: {c0}"),
            format!("d: {d0}"),
            format!("inner: {inner}"),
        ]);
        self.line(format!("let mut r = Rec {{ {} }};", fields.join(", ")));
        for k in ["r.a", "r.b", "r.inner.x", "r.inner.y"] {
            self.set_bound(k, 9);
        }
        self.set_bound("r.c", cb);
        self.print_rec();

        // arrays
        let elems: Vec<String> = (0..self.arr_len).map(|_| self.lit().to_string()).collect();
        self.line(format!("let mut a = [{}];", elems.join(", ")));
        self.set_bound("a", 9);
        let m: Vec<String> = (0..2)
            .map(|_| format!("[{}, {}]", self.lit(), self.lit()))
            .collect();
        self.line(format!("let mut m = [{}];", m.join(", ")));
        self.set_bound("m", 9);
        self.print_arrays();

        // scalars of every type
        let s0 = self.lit();
        self.line(format!("let mut s = {s0};"));
        self.set_bound("s", 9);
        let b0 = self.signed_lit();
        self.line(format!("let mut b: i64 = {b0};"));
        self.set_bound("b", 9);
        let (f0, fb) = self.float_lit();
        self.line(format!("let mut f = {f0};"));
        self.set_bound("f", fb);
        let c1 = self.letter();
        let c2 = self.letter();
        self.line(format!("let c1 = '{c1}';"));
        self.line(format!("let c2 = '{c2}';"));
        let w = self.word();
        self.line(format!("let t = \"{w}\";"));
        self.line("print_i32(s);");
        self.line("print_i64(b);");
        self.line("print_f64(f);");

        let n = self.rng.int(6, 12);
        for _ in 0..n {
            self.stmt();
        }
        self.fixed_tail();
        self.line("return s;");
        self.close();
    }

    fn print_rec(&mut self) {
        for f in [
            "print_i32(r.a);",
            "print_i64(r.b);",
            "print_f64(r.c);",
            "print_bool(r.d);",
            "print_i32(r.inner.x);",
            "print_i32(r.inner.y);",
        ] {
            self.line(f);
        }
    }

    fn print_arrays(&mut self) {
        for k in 0..self.arr_len {
            self.line(format!("print_i32(a[{k}]);"));
        }
        for i in 0..2 {
            for j in 0..2 {
                self.line(format!("print_i32(m[{i}][{j}]);"));
            }
        }
    }

    fn fixed_tail(&mut self) {
        // for variable shadowing an outer binding, printed after the loop
        self.open("for s in 0..3 {");
        self.line("print_i32(s);");
        self.close();
        self.line("print_i32(s);");
        // helpers taking aggregates by value; originals must be intact
        self.line("print_i32(poke(r));");
        self.print_rec();
        self.line("print_i32(fill(a));");
        self.line("print_i32(a[0]);");
        self.line("print_i32(flip(m));");
        self.line("print_i32(m[1][0]);");
        // char / string / i64 / f64 comparisons and string concatenation
        let l = self.letter();
        let w = self.word();
        self.line("print_bool(c1 < c2);");
        self.line("print_bool(c1 != c2);");
        self.line(format!("print_bool(c1 == '{l}');"));
        self.line(format!("print_bool(t == \"{w}\");"));
        self.line(format!("println(t + \"{w}\");"));
        self.line("print_i32(len(t + t));");
        let (e, _) = self.int_expr(Int::I64, 1);
        self.line(format!("print_bool(b < ({e}));"));
        self.line("print_bool((r.b) <= (b));");
        let (e, _) = self.f64_expr(1);
        self.line(format!("print_bool(f >= ({e}));"));
        self.line("print_bool((r.c) > (f));");
        self.casts();
    }

    // ----- statements -----

    fn stmt(&mut self) {
        match self.rng.int(0, 11) {
            0 => {
                let k = self.idx();
                let (e, eb) = self.int_expr(Int::I32, 2);
                self.line(format!("a[{k}] = {e};"));
                self.line(format!("print_i32(a[{k}]);"));
                let nb = self.bound("a").max(eb);
                self.set_bound("a", nb);
            }
            1 => {
                let i = self.rng.int(0, 1);
                let j = self.rng.int(0, 1);
                let (e, eb) = self.int_expr(Int::I32, 2);
                self.line(format!("m[{i}][{j}] = {e};"));
                self.line(format!("print_i32(m[{i}][{j}]);"));
                let nb = self.bound("m").max(eb);
                self.set_bound("m", nb);
            }
            2 => self.assign_field(false),
            3 => self.assign_scalar(false),
            4 => self.for_loop(),
            5 => self.while_loop(),
            6 => {
                let c = self.bool_expr(2);
                self.line(format!("print_bool({c});"));
            }
            7 => self.string_stmt(),
            8 => self.casts(),
            9 => {
                let c = self.bool_expr(2);
                self.open(format!("if ({c}) {{"));
                self.assign_scalar(true);
                self.else_branch();
                self.assign_field(true);
                self.close();
            }
            10 => {
                let (e, eb) = self.int_expr(Int::I32, 1);
                if eb + 9 < LIMIT {
                    self.line(format!("print_i32(bump({e}));"));
                }
                let (e, eb) = self.int_expr(Int::I64, 1);
                if eb * 2 < LIMIT {
                    self.line(format!("print_i64(twice({e}));"));
                }
            }
            _ => self.i64_stmt(),
        }
    }

    fn assign_scalar(&mut self, merge: bool) {
        match self.rng.int(0, 2) {
            0 => {
                let (e, eb) = self.int_expr(Int::I32, 2);
                self.line(format!("s = {e};"));
                self.line("print_i32(s);");
                let nb = if merge { self.bound("s").max(eb) } else { eb };
                self.set_bound("s", nb);
            }
            1 => {
                let (e, eb) = self.int_expr(Int::I64, 2);
                self.line(format!("b = {e};"));
                self.line("print_i64(b);");
                let nb = if merge { self.bound("b").max(eb) } else { eb };
                self.set_bound("b", nb);
            }
            _ => {
                let (e, eb) = self.f64_expr(2);
                self.line(format!("f = {e};"));
                self.line("print_f64(f);");
                let nb = if merge { self.bound("f").max(eb) } else { eb };
                self.set_bound("f", nb);
            }
        }
    }

    fn assign_field(&mut self, merge: bool) {
        match self.rng.int(0, 4) {
            0 => {
                let (e, eb) = self.int_expr(Int::I32, 2);
                self.line(format!("r.a = {e};"));
                self.line("print_i32(r.a);");
                let nb = if merge { self.bound("r.a").max(eb) } else { eb };
                self.set_bound("r.a", nb);
            }
            1 => {
                let key = *self.rng.choose(&["r.inner.x", "r.inner.y"]);
                let (e, eb) = self.int_expr(Int::I32, 2);
                self.line(format!("{key} = {e};"));
                self.line(format!("print_i32({key});"));
                let nb = if merge { self.bound(key).max(eb) } else { eb };
                self.set_bound(key, nb);
            }
            2 => {
                let (e, eb) = self.int_expr(Int::I64, 2);
                self.line(format!("r.b = {e};"));
                self.line("print_i64(r.b);");
                let nb = if merge { self.bound("r.b").max(eb) } else { eb };
                self.set_bound("r.b", nb);
            }
            3 => {
                let (e, eb) = self.f64_expr(2);
                self.line(format!("r.c = {e};"));
                self.line("print_f64(r.c);");
                let nb = if merge { self.bound("r.c").max(eb) } else { eb };
                self.set_bound("r.c", nb);
            }
            _ => {
                let c = self.bool_expr(1);
                self.line(format!("r.d = {c};"));
                self.line("print_bool(r.d);");
            }
        }
    }

    fn for_loop(&mut self) {
        let lo = self.rng.int(0, 2);
        let hi = lo + self.rng.int(0, 5);
        let iters = (hi - lo) as i64;
        let var = self.fresh("i");
        let n = self.arr_len;
        self.open(format!("for {var} in {lo}..{hi} {{"));
        self.loop_var = Some(var.clone());
        let count = self.rng.int(1, 3);
        for _ in 0..count {
            match self.rng.int(0, 5) {
                0 => {
                    let (e, eb) = self.small_i32();
                    let nb = self.bound("s") + iters * eb;
                    if nb < LIMIT {
                        self.line(format!("s = s + ({e});"));
                        self.set_bound("s", nb);
                    }
                    self.line("print_i32(s);");
                }
                1 => {
                    let k = self.idx();
                    let nb = self.bound("a") + iters * 7;
                    if nb < LIMIT {
                        self.line(format!("a[{k}] = a[{k}] + {var};"));
                        self.set_bound("a", nb);
                    }
                    self.line(format!("print_i32(a[{k}]);"));
                }
                2 => self.line(format!("print_i32({var});")),
                3 => {
                    // right side only in bounds thanks to short-circuit
                    self.open(format!("if ({var} < {n} && a[{var}] > 0) {{"));
                    self.line(format!("print_i32(a[{var}]);"));
                    self.else_branch();
                    self.line("println(\"skip\");");
                    self.close();
                }
                4 => {
                    let l = self.lit();
                    self.open(format!("if ({var} >= {n} || a[{var}] == {l}) {{"));
                    self.line("println(\"hit\");");
                    self.else_branch();
                    self.line(format!("print_i32(a[{var}]);"));
                    self.close();
                }
                _ => {
                    let l = self.rng.int(0, 6);
                    let kw = if self.rng.bool() { "break" } else { "continue" };
                    self.line(format!("if ({var} == {l}) {{ {kw}; }}"));
                }
            }
        }
        self.loop_var = None;
        self.close();
    }

    fn while_loop(&mut self) {
        let w = self.fresh("w");
        let lim = self.rng.int(1, 6);
        self.line(format!("let mut {w} = 0;"));
        self.open(format!("while ({w} < {lim}) {{"));
        self.line(format!("{w} = {w} + 1;"));
        let k = self.rng.int(0, 3) as i64;
        let nb = self.bound("s") + lim as i64 * k;
        if nb < LIMIT {
            self.line(format!("s = s + {k};"));
            self.set_bound("s", nb);
        }
        self.line(format!("print_i32({w});"));
        self.close();
        self.line(format!("print_i32({w});"));
        self.line("print_i32(s);");
    }

    fn string_stmt(&mut self) {
        let w = self.word();
        match self.rng.int(0, 2) {
            0 => self.line(format!("println(t + \"{w}\");")),
            1 => {
                let u = self.fresh("t");
                self.line(format!("let {u} = t + t;"));
                self.line(format!("println({u});"));
                self.line(format!("print_i32(len({u}));"));
                self.line(format!("print_bool({u} == t);"));
            }
            _ => self.line(format!("print_bool(t == \"{w}\");")),
        }
    }

    fn i64_stmt(&mut self) {
        match self.rng.int(0, 3) {
            0 => {
                self.line("b = -(b);");
                self.line("print_i64(b);");
            }
            1 => {
                // negative literal adopting the annotated i64 type
                let n = self.fresh("n");
                let l = self.lit();
                self.line(format!("let {n}: i64 = -{l};"));
                self.line(format!("print_i64({n});"));
                self.line(format!("print_bool({n} < b);"));
            }
            2 => {
                // literal on the left: its type comes from the right operand
                let l = self.lit();
                let nb = self.bound("b") + l;
                if nb < LIMIT {
                    self.line(format!("b = {l} + b;"));
                    self.set_bound("b", nb);
                }
                self.line("print_i64(b);");
            }
            _ => self.line("print_i64(-(b));"),
        }
    }

    fn casts(&mut self) {
        let count = self.rng.int(1, 3);
        for _ in 0..count {
            match self.rng.int(0, 10) {
                0 => {
                    let (e, _) = self.int_expr(Int::I32, 1);
                    self.line(format!("print_i64(({e}) as i64);"));
                }
                1 => {
                    let (e, _) = self.int_expr(Int::I32, 1);
                    self.line(format!("print_f64(({e}) as f64);"));
                }
                2 => {
                    let (e, _) = self.int_expr(Int::I64, 1);
                    self.line(format!("print_f64(({e}) as f64);"));
                }
                3 => {
                    let c = self.bool_expr(1);
                    self.line(format!("print_i32(({c}) as i32);"));
                }
                4 => {
                    let c = self.bool_expr(1);
                    self.line(format!("print_i64(({c}) as i64);"));
                }
                5 => {
                    let c = *self.rng.choose(&["c1", "c2"]);
                    self.line(format!("print_i32(({c}) as i32);"));
                }
                6 => {
                    let k = self.rng.int(0, 25);
                    self.line(format!("print_i32(((65 + {k}) as char) as i32);"));
                }
                7 => {
                    let (e, _) = self.f64_expr(1);
                    self.line(format!("print_i32(({e}) as i32);"));
                }
                8 => {
                    let (e, _) = self.f64_expr(1);
                    self.line(format!("print_i64(({e}) as i64);"));
                }
                9 => {
                    let (e, _) = self.int_expr(Int::I64, 1);
                    self.line(format!("print_i32(({e}) as i32);"));
                }
                _ => {
                    let ch = self.fresh("ch");
                    let code = self.rng.int(65, 90);
                    self.line(format!("let {ch} = ({code}) as char;"));
                    self.line(format!("print_bool({ch} == 'A');"));
                    self.line(format!("print_bool({ch} < c1);"));
                    self.line(format!("print_i32(({ch}) as i32);"));
                }
            }
        }
    }

    // ----- expressions -----

    /// Loop-body increment: bound at most 7.
    fn small_i32(&mut self) -> (String, i64) {
        match self.rng.int(0, 2) {
            0 => {
                let v = self.rng.int(0, 3) as i64;
                (v.to_string(), v)
            }
            1 => match self.loop_var.clone() {
                Some(v) => (v, 7),
                None => ("1".into(), 1),
            },
            _ => {
                let k = self.rng.int(0, 2);
                (format!("({k} * 2)"), k as i64 * 2)
            }
        }
    }

    fn int_expr(&mut self, ty: Int, depth: u32) -> (String, i64) {
        if depth == 0 || self.rng.int(0, 2) == 0 {
            return self.int_leaf(ty);
        }
        let (l, lb) = self.int_expr(ty, depth - 1);
        let (text, b) = match self.rng.int(0, 3) {
            0 => {
                let (r, rb) = self.int_expr(ty, depth - 1);
                (format!("({l} + {r})"), lb + rb)
            }
            1 => {
                let (r, rb) = self.int_expr(ty, depth - 1);
                (format!("({l} - {r})"), lb + rb)
            }
            2 => {
                let k = self.rng.int(0, 3) as i64;
                if self.rng.bool() {
                    (format!("({l} * {k})"), lb * k)
                } else {
                    (format!("({k} * {l})"), lb * k)
                }
            }
            _ => (format!("(-{l})"), lb),
        };
        if b >= LIMIT {
            self.int_leaf(ty)
        } else {
            (text, b)
        }
    }

    fn int_leaf(&mut self, ty: Int) -> (String, i64) {
        let (text, b) = match ty {
            Int::I32 => match self.rng.int(0, 7) {
                0 | 1 => {
                    let v = self.lit();
                    (v.to_string(), v)
                }
                2 => ("s".to_string(), self.bound("s")),
                3 => {
                    let k = self.idx();
                    (format!("a[{k}]"), self.bound("a"))
                }
                4 => {
                    let i = self.rng.int(0, 1);
                    let j = self.rng.int(0, 1);
                    (format!("m[{i}][{j}]"), self.bound("m"))
                }
                5 => {
                    let f = *self.rng.choose(&["r.a", "r.inner.x", "r.inner.y"]);
                    (f.to_string(), self.bound(f))
                }
                6 => match self.loop_var.clone() {
                    Some(v) => (v, 7),
                    None => ("s".to_string(), self.bound("s")),
                },
                _ => ("bump(s)".to_string(), self.bound("s") + 9),
            },
            Int::I64 => match self.rng.int(0, 6) {
                0 | 1 => {
                    let v = self.lit();
                    (v.to_string(), v)
                }
                2 => ("b".to_string(), self.bound("b")),
                3 => ("r.b".to_string(), self.bound("r.b")),
                4 => ("(s) as i64".to_string(), self.bound("s")),
                5 => ("(r.a) as i64".to_string(), self.bound("r.a")),
                _ => ("twice(b)".to_string(), self.bound("b") * 2),
            },
        };
        if b >= LIMIT {
            let v = self.lit();
            (v.to_string(), v)
        } else {
            (text, b)
        }
    }

    fn f64_expr(&mut self, depth: u32) -> (String, i64) {
        if depth == 0 || self.rng.int(0, 2) == 0 {
            return self.f64_leaf();
        }
        let (l, lb) = self.f64_expr(depth - 1);
        let (text, b) = match self.rng.int(0, 3) {
            0 => {
                let (r, rb) = self.f64_expr(depth - 1);
                (format!("({l} + {r})"), lb + rb)
            }
            1 => {
                let (r, rb) = self.f64_expr(depth - 1);
                (format!("({l} - {r})"), lb + rb)
            }
            2 => {
                let k = self.rng.int(0, 3) as i64;
                (format!("({l} * {k}.0)"), lb * k)
            }
            _ => (format!("(-{l})"), lb),
        };
        if b >= LIMIT {
            self.f64_leaf()
        } else {
            (text, b)
        }
    }

    fn f64_leaf(&mut self) -> (String, i64) {
        let (text, b) = match self.rng.int(0, 5) {
            0 | 1 => self.float_lit(),
            2 => ("f".to_string(), self.bound("f")),
            3 => ("r.c".to_string(), self.bound("r.c")),
            4 => ("(s) as f64".to_string(), self.bound("s")),
            _ => ("(b) as f64".to_string(), self.bound("b")),
        };
        if b >= LIMIT {
            self.float_lit()
        } else {
            (text, b)
        }
    }

    fn bool_expr(&mut self, depth: u32) -> String {
        let choice = if depth == 0 { self.rng.int(0, 6) } else { self.rng.int(0, 8) };
        match choice {
            0 => "r.d".into(),
            1 => self.rng.bool().to_string(),
            2 => {
                let (l, _) = self.int_expr(Int::I32, 1);
                let (r, _) = self.int_expr(Int::I32, 1);
                let op = *self.rng.choose(CMP);
                format!("({l}) {op} ({r})")
            }
            3 => {
                let (l, _) = self.int_expr(Int::I64, 1);
                let (r, _) = self.int_expr(Int::I64, 1);
                let op = *self.rng.choose(CMP);
                format!("({l}) {op} ({r})")
            }
            4 => {
                let (l, _) = self.f64_expr(1);
                let (r, _) = self.f64_expr(1);
                let op = *self.rng.choose(&["<", "<=", ">", ">=", "!="]);
                format!("({l}) {op} ({r})")
            }
            5 => {
                let op = *self.rng.choose(CMP);
                if self.rng.bool() {
                    format!("c1 {op} c2")
                } else {
                    let l = self.letter();
                    format!("c2 {op} '{l}'")
                }
            }
            6 => {
                let w = self.word();
                let op = if self.rng.bool() { "==" } else { "!=" };
                format!("t {op} \"{w}\"")
            }
            7 => format!("!({})", self.bool_expr(depth - 1)),
            _ => {
                let l = self.bool_expr(depth - 1);
                let r = self.bool_expr(depth - 1);
                let op = if self.rng.bool() { "&&" } else { "||" };
                format!("({l}) {op} ({r})")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_program_is_deterministic() {
        let a = gen_aggregate_program(&mut FuzzRng::new(5));
        let b = gen_aggregate_program(&mut FuzzRng::new(5));
        assert_eq!(a, b);
        assert!(a.contains("struct Rec"));
        assert!(a.contains("fn main() -> i32"));
        assert!(a.contains("poke(r)"));
        assert!(a.contains("return s;"));
    }

    #[test]
    fn aggregate_programs_vary_with_seed() {
        let a = gen_aggregate_program(&mut FuzzRng::new(1));
        let b = gen_aggregate_program(&mut FuzzRng::new(2));
        assert_ne!(a, b);
    }
}

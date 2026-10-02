//! Language-feature program generator (`--kind lang`).
//!
//! Produces well-typed, always-terminating, runtime-error-free programs that
//! exercise the 0.3 surface: enums with 0..3 payload variants, exhaustive
//! `match` statements, `if let ... else`, tuples (nested, `.0` access, field
//! assignment, `let (a, b) = t;`, `==`), compound assignment on every kind
//! of place, bitwise / shift operators (shift amounts of any size, since
//! they are masked), hex / binary / underscore / `\u{..}` literals, the
//! numeric and string built-ins, `yield;`, functions that take and return
//! structs / tuples / enums / arrays by value and mutate their copy, and
//! multi-file programs (`use`, import cycles, `pub` on every imported item).
//!
//! Oracles: every value is printed, so stdout is the differential oracle;
//! on top of that the program checks value semantics itself and prints
//! `VIOLATION` when a copy aliases its source.
//!
//! Integer overflow is wrapping on every level, so it is generated freely
//! (`i32::MAX` literals, shifts by 40, `pow_i32`); divisors are non-zero
//! literals, array indices are literals or `(e & (n - 1))`, loops have
//! literal bounds and helpers only call earlier helpers.

use super::rng::FuzzRng;

/// Generate `match` as an expression (`(match (e) & 3 { 0 => a, 1 => b, _ => c })`)
/// inside other expressions. On since `match` became an expression (0.3.0).
pub const ALLOW_MATCH_EXPR: bool = true;

#[derive(Clone, PartialEq, Debug)]
enum Ty {
    I32,
    I64,
    F64,
    Bool,
    Char,
    Str,
    Tup(Vec<Ty>),
    Struct(usize),
    Enum(usize),
    Arr(Box<Ty>, usize),
}

struct StructDef {
    name: String,
    fields: Vec<(String, Ty)>,
}

struct EnumDef {
    name: String,
    variants: Vec<(String, Vec<Ty>)>,
}

struct FnSig {
    name: String,
    params: Vec<Ty>,
    ret: Option<Ty>,
}

struct Var {
    name: String,
    ty: Ty,
    assignable: bool,
}

struct Snap {
    var: String,
    suffix: String,
    ty: Ty,
}

#[derive(Clone)]
struct Place {
    path: String,
    ty: Ty,
    assignable: bool,
}

/// A generated program: `files[0]` is the main file; names are relative
/// paths (they may contain a directory component).
pub struct LangProgram {
    pub files: Vec<(String, String)>,
}

impl LangProgram {
    pub fn is_single(&self) -> bool {
        self.files.len() == 1
    }
}

pub fn gen_lang_program(rng: &mut FuzzRng) -> LangProgram {
    let (items, main) = {
        let mut g = G::new(rng);
        g.build()
    };
    layout(rng, items, main)
}

/// Single-file variant (every item in one source), for the oracles that
/// work on text.
pub fn gen_lang_source(rng: &mut FuzzRng) -> String {
    let (items, main) = {
        let mut g = G::new(rng);
        g.build()
    };
    let mut s = String::new();
    for it in items {
        s.push_str(&it);
        s.push('\n');
    }
    s.push_str(&main);
    s
}

fn import_path(rng: &mut FuzzRng, rel: &str) -> String {
    // exercises the optional `.ae` extension and a leading `./`
    let mut p = rel.to_string();
    if rng.int(0, 3) == 0 {
        p = p.trim_end_matches(".ae").to_string();
    }
    if rng.int(0, 3) == 0 {
        p = format!("./{p}");
    }
    p
}

fn layout(rng: &mut FuzzRng, items: Vec<String>, main: String) -> LangProgram {
    let multi = items.len() >= 3 && rng.int(0, 99) < 45;
    if !multi {
        let mut s = String::new();
        for it in items {
            if rng.int(0, 2) == 0 {
                s.push_str("pub ");
            }
            s.push_str(&it);
            s.push('\n');
        }
        s.push_str(&main);
        return LangProgram {
            files: vec![("main.ae".into(), s)],
        };
    }
    // Segments in definition order: library a, library b, items kept in main.
    // Items only reference earlier items, so imports point backwards (and
    // optionally forwards, which makes a cycle).
    let n = items.len();
    let c1 = rng.int(1, (n - 1) as i32) as usize;
    let c2 = rng.int(c1 as i32, n as i32) as usize;
    let c2 = if rng.int(0, 2) == 0 { n } else { c2 };
    let two_libs = c2 > c1 && c1 < n && rng.bool();
    let (a_items, b_items, m_items) = if two_libs {
        (&items[..c1], &items[c1..c2], &items[c2..])
    } else {
        (&items[..c2.max(c1)], &items[0..0], &items[c2.max(c1)..])
    };
    let dir = if rng.int(0, 99) < 40 { "lib/" } else { "" };
    let a_name = format!("{dir}a.ae");
    let b_name = format!("{dir}b.ae");
    let cycle = rng.bool();
    let mut files: Vec<(String, String)> = Vec::new();

    let mut main_text = String::new();
    main_text.push_str(&format!("use \"{}\";\n", import_path(rng, &a_name)));
    if two_libs {
        main_text.push_str(&format!("use \"{}\";\n", import_path(rng, &b_name)));
    }
    for it in m_items {
        if rng.int(0, 2) == 0 {
            main_text.push_str("pub ");
        }
        main_text.push_str(it);
        main_text.push('\n');
    }
    main_text.push_str(&main);

    let mut a_text = String::new();
    if two_libs && cycle {
        // a -> b -> a import cycle
        a_text.push_str(&format!("use \"{}\";\n", import_path(rng, "b.ae")));
    }
    if !dir.is_empty() && rng.int(0, 2) == 0 {
        // cycle through the entry file itself
        a_text.push_str("use \"../main.ae\";\n");
    }
    for it in a_items {
        a_text.push_str("pub ");
        a_text.push_str(it);
        a_text.push('\n');
    }
    files.push(("main.ae".into(), main_text));
    files.push((a_name, a_text));
    if two_libs {
        let mut b_text = format!("use \"{}\";\n", import_path(rng, "a.ae"));
        for it in b_items {
            b_text.push_str("pub ");
            b_text.push_str(it);
            b_text.push('\n');
        }
        files.push((b_name, b_text));
    }
    LangProgram { files }
}

const WORDS: &[&str] = &[
    "", "ab", "hello", "a\\tb", "caf\\u{e9}", "\\u{1F600}x", "q\\\"q", "back\\\\slash", "x\\ny",
    "aether", "\\u{41}\\u{42}",
];
const CHARS: &[&str] = &[
    "'a'", "'Z'", "'0'", "'\\n'", "'\\u{e9}'", "'\\u{1F600}'", "'\\t'", "'\\''", "'\\\\'",
    "'\\u{41}'", "' '",
];
const FOUR: &[&str] = &["abcd", "w\\u{e9}yz", "\\u{1F600}b\\u{41}d", "0123"];
const CMP: &[&str] = &["<", "<=", ">", ">=", "==", "!="];

struct G<'r> {
    rng: &'r mut FuzzRng,
    out: String,
    indent: usize,
    structs: Vec<StructDef>,
    enums: Vec<EnumDef>,
    fns: Vec<FnSig>,
    env: Vec<Var>,
    fresh: u32,
    depth: u32,
    loop_depth: u32,
    in_loop: bool,
    in_helper: bool,
    ret_ty: Option<Ty>,
    /// No helper calls (their prints would run twice in `a[f()] += 1`).
    no_call: bool,
    /// No literal outside the `i32` range (the type may come from `i32`).
    no_big: bool,
    /// Array literals do not propagate the element type: cast `i64`
    /// subexpressions from `i32` instead of relying on a bare literal.
    force: bool,
}

impl<'r> G<'r> {
    fn new(rng: &'r mut FuzzRng) -> Self {
        G {
            rng,
            out: String::new(),
            indent: 0,
            structs: Vec::new(),
            enums: Vec::new(),
            fns: Vec::new(),
            env: Vec::new(),
            fresh: 0,
            depth: 0,
            loop_depth: 0,
            in_loop: false,
            in_helper: false,
            ret_ty: None,
            no_call: false,
            no_big: false,
            force: false,
        }
    }

    // ----- emission -----

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

    fn close(&mut self) {
        self.indent -= 1;
        self.line("}");
    }

    fn else_open(&mut self) {
        self.indent -= 1;
        self.line("} else {");
        self.indent += 1;
    }

    fn fresh(&mut self, p: &str) -> String {
        self.fresh += 1;
        // the underscore keeps names clear of keywords and types (`i32`, `f64`)
        format!("{p}_{}", self.fresh)
    }

    fn chance(&mut self, pct: i32) -> bool {
        self.rng.int(0, 99) < pct
    }

    fn pick(&mut self, n: usize) -> usize {
        self.rng.int(0, n as i32 - 1) as usize
    }

    fn take_out(&mut self) -> String {
        self.indent = 0;
        std::mem::take(&mut self.out)
    }

    // ----- types -----

    fn scalar_ty(&mut self) -> Ty {
        match self.rng.int(0, 9) {
            0..=3 => Ty::I32,
            4 | 5 => Ty::I64,
            6 => Ty::F64,
            7 => Ty::Bool,
            8 => Ty::Char,
            _ => Ty::Str,
        }
    }

    fn rand_ty(&mut self, depth: u32) -> Ty {
        if depth == 0 {
            return self.scalar_ty();
        }
        match self.rng.int(0, 11) {
            0..=5 => self.scalar_ty(),
            6 | 7 => {
                let n = self.rng.int(2, 3);
                Ty::Tup((0..n).map(|_| self.rand_ty(depth - 1)).collect())
            }
            8 if !self.structs.is_empty() => Ty::Struct(self.pick(self.structs.len())),
            9 if !self.enums.is_empty() => Ty::Enum(self.pick(self.enums.len())),
            10 | 11 => {
                let n = if self.rng.bool() { 2 } else { 4 };
                Ty::Arr(Box::new(self.rand_ty(depth - 1)), n)
            }
            _ => self.scalar_ty(),
        }
    }

    fn ty_name(&self, t: &Ty) -> String {
        match t {
            Ty::I32 => "i32".into(),
            Ty::I64 => "i64".into(),
            Ty::F64 => "f64".into(),
            Ty::Bool => "bool".into(),
            Ty::Char => "char".into(),
            Ty::Str => "string".into(),
            Ty::Tup(es) => format!(
                "({})",
                es.iter().map(|e| self.ty_name(e)).collect::<Vec<_>>().join(", ")
            ),
            Ty::Struct(i) => self.structs[*i].name.clone(),
            Ty::Enum(i) => self.enums[*i].name.clone(),
            Ty::Arr(e, n) => format!("[{}; {n}]", self.ty_name(e)),
        }
    }

    fn has_i64(&self, t: &Ty) -> bool {
        match t {
            Ty::I64 => true,
            Ty::Tup(es) => es.iter().any(|e| self.has_i64(e)),
            Ty::Struct(i) => self.structs[*i].fields.iter().any(|(_, f)| self.has_i64(f)),
            Ty::Arr(e, _) => self.has_i64(e),
            _ => false,
        }
    }

    /// `==` support: arrays (anywhere inside) are not comparable.
    fn eq_ok(&self, t: &Ty) -> bool {
        match t {
            Ty::Arr(..) => false,
            Ty::Tup(es) => es.iter().all(|e| self.eq_ok(e)),
            Ty::Struct(i) => self.structs[*i].fields.iter().all(|(_, f)| self.eq_ok(f)),
            Ty::Enum(i) => self
                .enums[*i]
                .variants
                .iter()
                .all(|(_, ps)| ps.iter().all(|p| self.eq_ok(p))),
            _ => true,
        }
    }

    fn is_agg(t: &Ty) -> bool {
        matches!(t, Ty::Tup(_) | Ty::Struct(_) | Ty::Enum(_) | Ty::Arr(..))
    }

    // ----- places -----

    fn collect(&self, path: String, ty: &Ty, assignable: bool, out: &mut Vec<Place>) {
        out.push(Place {
            path: path.clone(),
            ty: ty.clone(),
            assignable,
        });
        match ty {
            Ty::Tup(es) => {
                for (i, e) in es.iter().enumerate() {
                    self.collect(format!("{path}.{i}"), e, assignable, out);
                }
            }
            Ty::Struct(si) => {
                for (n, t) in &self.structs[*si].fields {
                    self.collect(format!("{path}.{n}"), t, assignable, out);
                }
            }
            Ty::Arr(e, n) => {
                for k in 0..*n {
                    self.collect(format!("{path}[{k}]"), e, assignable, out);
                }
            }
            _ => {}
        }
    }

    fn all_places(&self) -> Vec<Place> {
        let mut out = Vec::new();
        for v in &self.env {
            self.collect(v.name.clone(), &v.ty, v.assignable, &mut out);
        }
        out
    }

    fn var_places(&self, name: &str) -> Vec<Place> {
        let mut out = Vec::new();
        for v in self.env.iter().filter(|v| v.name == name) {
            self.collect(v.name.clone(), &v.ty, v.assignable, &mut out);
        }
        out
    }

    /// A readable place of type `want`, possibly an array element with a
    /// dynamic (masked) index.
    fn leaf(&mut self, want: &Ty) -> Option<String> {
        let places = self.all_places();
        let direct: Vec<&Place> = places.iter().filter(|p| &p.ty == want).collect();
        let arrays: Vec<&Place> = places
            .iter()
            .filter(|p| matches!(&p.ty, Ty::Arr(e, _) if **e == *want))
            .collect();
        let total = direct.len() + arrays.len();
        if total == 0 {
            return None;
        }
        let k = self.pick(total);
        if k < direct.len() {
            return Some(direct[k].path.clone());
        }
        let p = arrays[k - direct.len()];
        let n = match &p.ty {
            Ty::Arr(_, n) => *n,
            _ => unreachable!(),
        };
        let path = p.path.clone();
        Some(self.dyn_index(&path, n))
    }

    /// Index expression with no helper calls and no nested dynamic index
    /// (an index may be evaluated twice by a compound assignment).
    fn idx_expr(&mut self) -> String {
        let places = self.all_places();
        let direct: Vec<&Place> = places.iter().filter(|p| p.ty == Ty::I32).collect();
        let base = if !direct.is_empty() && self.chance(65) {
            direct[self.pick(direct.len())].path.clone()
        } else {
            self.int_lit(&Ty::I32)
        };
        match self.rng.int(0, 3) {
            0 => format!("({base} + {})", self.rng.int(0, 9)),
            1 => format!("({base} * 3)"),
            _ => base,
        }
    }

    fn dyn_index(&mut self, path: &str, n: usize) -> String {
        let e = self.idx_expr();
        format!("{path}[(({e}) & {})]", n - 1)
    }

    // ----- literals -----

    fn fmt_u(&mut self, v: u64) -> String {
        match self.rng.int(0, 7) {
            0 => format!("0x{v:X}"),
            1 => format!("0x{v:x}"),
            2 => format!("0b{v:b}"),
            3 => {
                // underscore-separated decimal
                let s = v.to_string();
                let mut out = String::new();
                for (i, c) in s.chars().enumerate() {
                    if i > 0 && (s.len() - i) % 3 == 0 {
                        out.push('_');
                    }
                    out.push(c);
                }
                out
            }
            4 if v > 0xFF => {
                let h = format!("{v:X}");
                let mid = h.len() / 2;
                format!("0x{}_{}", &h[..mid], &h[mid..])
            }
            _ => v.to_string(),
        }
    }

    fn int_lit(&mut self, ty: &Ty) -> String {
        let neg = self.chance(25);
        let v: u64 = match self.rng.int(0, 11) {
            0..=4 => self.rng.int(0, 9) as u64,
            5 | 6 => self.rng.int(10, 300) as u64,
            7 => self.rng.int(0, 65535) as u64,
            8 => 1u64 << self.rng.int(0, 30),
            9 => *self.rng.choose(&[2147483647u64, 2147483647, 65535, 255, 4096]),
            10 if *ty == Ty::I64 && !self.no_big && !self.force => {
                *self.rng.choose(&[
                    4294967296u64,
                    4294967295,
                    1u64 << 40,
                    9223372036854775807,
                    1u64 << 62,
                ])
            }
            _ => self.rng.int(0, 99) as u64,
        };
        if neg && v != 0 {
            if v == 2147483647 && *ty == Ty::I32 && self.rng.bool() {
                return "(-2147483648)".into();
            }
            format!("(-{})", self.fmt_u(v))
        } else {
            self.fmt_u(v)
        }
    }

    fn float_lit(&mut self) -> String {
        let w = self.rng.int(0, 9);
        let f = self.rng.int(0, 9);
        match self.rng.int(0, 5) {
            0 => format!("{w}.0"),
            1 => format!("{}.{f}", w * 100 + 7),
            2 => format!("1_{w}00.{f}"),
            3 => "0.125".into(),
            _ => format!("{w}.{f}"),
        }
    }

    fn word(&mut self) -> String {
        format!("\"{}\"", *self.rng.choose(WORDS))
    }

    // ----- expressions -----

    fn expr(&mut self, ty: &Ty, d: u32) -> String {
        match ty {
            Ty::I32 | Ty::I64 => self.int_expr(ty, d),
            Ty::F64 => self.f64_expr(d),
            Ty::Bool => self.bool_expr(d),
            Ty::Char => self.char_expr(d),
            Ty::Str => self.str_expr(d),
            _ => self.agg_expr(ty, d),
        }
    }

    /// A readable place or a helper call of type `ty`.
    fn reuse(&mut self, ty: &Ty, d: u32) -> Option<String> {
        let r = self.rng.int(0, 9);
        if r < 3 {
            return self.leaf(ty);
        }
        if r == 3 && d > 0 {
            return self.call(ty, d);
        }
        None
    }

    fn call(&mut self, ty: &Ty, d: u32) -> Option<String> {
        if self.no_call {
            return None;
        }
        let cands: Vec<usize> = self
            .fns
            .iter()
            .enumerate()
            .filter(|(_, f)| f.ret.as_ref() == Some(ty))
            .map(|(i, _)| i)
            .collect();
        if cands.is_empty() {
            return None;
        }
        let i = cands[self.pick(cands.len())];
        Some(self.call_text(i, d))
    }

    fn call_text(&mut self, i: usize, d: u32) -> String {
        let name = self.fns[i].name.clone();
        let params = self.fns[i].params.clone();
        let args: Vec<String> = params
            .iter()
            .map(|p| self.expr(p, d.saturating_sub(1)))
            .collect();
        format!("{name}({})", args.join(", "))
    }

    fn int_expr(&mut self, ty: &Ty, d: u32) -> String {
        if *ty == Ty::I64 && self.force {
            if self.chance(40) {
                if let Some(l) = self.leaf(ty) {
                    return l;
                }
            }
            let saved = self.force;
            self.force = false;
            let inner = self.int_expr(&Ty::I32, d);
            self.force = saved;
            return format!("(({inner}) as i64)");
        }
        if let Some(s) = self.reuse(ty, d) {
            return s;
        }
        if d == 0 {
            return self.int_leaf(ty);
        }
        let is32 = *ty == Ty::I32;
        match self.rng.int(0, 17) {
            0..=2 => {
                let op = *self.rng.choose(&["+", "-", "*"]);
                let l = self.int_expr(ty, d - 1);
                let r = self.int_expr(ty, d - 1);
                format!("({l} {op} {r})")
            }
            3 | 4 => {
                let op = *self.rng.choose(&["&", "|", "^"]);
                let l = self.int_expr(ty, d - 1);
                let r = self.int_expr(ty, d - 1);
                format!("({l} {op} {r})")
            }
            5 | 6 => {
                let op = if self.rng.bool() { "<<" } else { ">>" };
                let l = self.int_expr(ty, d - 1);
                let r = if self.chance(65) {
                    // literal amounts, including >= width and (via
                    // negation) negative ones: all masked
                    let k = *self.rng.choose(&[0, 1, 3, 7, 15, 31, 32, 33, 40, 63, 64, 70]);
                    if self.chance(15) {
                        format!("(-{k})")
                    } else {
                        k.to_string()
                    }
                } else {
                    self.int_expr(ty, d - 1)
                };
                format!("({l} {op} {r})")
            }
            7 => {
                let l = self.int_expr(ty, d - 1);
                // `-(-2147483648)` is rejected by the compiler (E0263, see docs/fuzzing.md)
                if self.rng.bool() && l != "(-2147483648)" {
                    format!("(-{l})")
                } else {
                    format!("(!{l})")
                }
            }
            8 => {
                let op = if self.rng.bool() { "/" } else { "%" };
                let l = self.int_expr(ty, d - 1);
                let k = *self.rng.choose(&["1", "2", "3", "5", "7", "(-1)", "(-3)", "16"]);
                format!("({l} {op} {k})")
            }
            9 | 10 if is32 => self.builtin_i32(d),
            9 | 10 => {
                let e = self.int_expr(&Ty::I32, d - 1);
                format!("(({e}) as i64)")
            }
            11 => {
                if is32 {
                    // a cast operand has no expected type: no big literals
                    let saved = self.no_big;
                    self.no_big = true;
                    let e = self.int_expr(&Ty::I64, d - 1);
                    self.no_big = saved;
                    format!("(({e}) as i32)")
                } else {
                    let e = self.int_expr(&Ty::I32, d - 1);
                    format!("(({e}) as i64)")
                }
            }
            12 => {
                let saved = self.no_big;
                self.no_big = true;
                let r = match self.rng.int(0, 2) {
                    0 => {
                        let b = self.bool_expr(d - 1);
                        format!("(({b}) as {})", if is32 { "i32" } else { "i64" })
                    }
                    1 => {
                        let f = self.f64_expr(d - 1);
                        format!("(({f}) as {})", if is32 { "i32" } else { "i64" })
                    }
                    _ if is32 => {
                        let c = self.char_expr(d - 1);
                        format!("(({c}) as i32)")
                    }
                    _ => {
                        let b = self.bool_expr(d - 1);
                        format!("(({b}) as i64)")
                    }
                };
                self.no_big = saved;
                r
            }
            13 if is32 => {
                if self.rng.bool() {
                    let s = self.str_expr(d - 1);
                    format!("len({s})")
                } else {
                    let arrs: Vec<Place> = self
                        .all_places()
                        .into_iter()
                        .filter(|p| matches!(p.ty, Ty::Arr(..)))
                        .collect();
                    if arrs.is_empty() {
                        let s = self.str_expr(d - 1);
                        format!("len({s})")
                    } else {
                        let k = self.pick(arrs.len());
                        format!("len({})", arrs[k].path)
                    }
                }
            }
            14 if ALLOW_MATCH_EXPR => self.match_expr(ty, d),
            _ => self.int_leaf(ty),
        }
    }

    fn builtin_i32(&mut self, d: u32) -> String {
        match self.rng.int(0, 5) {
            0 => format!("abs({})", self.int_expr(&Ty::I32, d - 1)),
            1 => format!(
                "min({}, {})",
                self.int_expr(&Ty::I32, d - 1),
                self.int_expr(&Ty::I32, d - 1)
            ),
            2 => format!(
                "max({}, {})",
                self.int_expr(&Ty::I32, d - 1),
                self.int_expr(&Ty::I32, d - 1)
            ),
            3 => {
                // lo <= hi is not required by the spec (max(lo, min(hi, x)))
                let x = self.int_expr(&Ty::I32, d - 1);
                let lo = self.int_expr(&Ty::I32, d - 1);
                let hi = self.int_expr(&Ty::I32, d - 1);
                format!("clamp({x}, {lo}, {hi})")
            }
            4 => {
                let b = self.int_expr(&Ty::I32, d - 1);
                let e = self.int_expr(&Ty::I32, d - 1);
                format!("pow_i32({b}, (({e}) & 7))")
            }
            _ => {
                let b = self.rng.int(-3, 12);
                let e = self.rng.int(0, 31);
                format!("pow_i32({}, {e})", if b < 0 { format!("(-{})", -b) } else { b.to_string() })
            }
        }
    }

    fn int_leaf(&mut self, ty: &Ty) -> String {
        if self.chance(55) {
            if let Some(p) = self.leaf(ty) {
                return p;
            }
        }
        self.int_lit(ty)
    }

    fn f64_expr(&mut self, d: u32) -> String {
        if let Some(s) = self.reuse(&Ty::F64, d) {
            return s;
        }
        if d == 0 {
            return self.f64_leaf();
        }
        match self.rng.int(0, 11) {
            0 | 1 => {
                let op = if self.rng.bool() { "+" } else { "-" };
                let l = self.f64_expr(d - 1);
                let r = self.f64_expr(d - 1);
                format!("({l} {op} {r})")
            }
            2 => {
                let l = self.f64_expr(d - 1);
                let k = self.rng.int(0, 3);
                format!("({l} * {k}.0)")
            }
            3 => {
                let l = self.f64_expr(d - 1);
                let k = self.rng.int(1, 7);
                format!("({l} / {k}.0)")
            }
            4 => format!("(-{})", self.f64_expr(d - 1)),
            5 => {
                let l = self.f64_expr(d - 1);
                format!("sqrt(({l} * {l}))")
            }
            6 => format!("floor({})", self.f64_expr(d - 1)),
            7 => format!("ceil({})", self.f64_expr(d - 1)),
            8 => {
                let saved = self.no_big;
                self.no_big = true;
                let e = self.int_expr(&Ty::I32, d - 1);
                self.no_big = saved;
                format!("(({e}) as f64)")
            }
            9 => {
                let saved = self.no_big;
                self.no_big = true;
                let e = self.int_expr(&Ty::I64, d - 1);
                self.no_big = saved;
                format!("(({e}) as f64)")
            }
            _ => self.f64_leaf(),
        }
    }

    fn f64_leaf(&mut self) -> String {
        if self.chance(50) {
            if let Some(p) = self.leaf(&Ty::F64) {
                return p;
            }
        }
        self.float_lit()
    }

    fn char_expr(&mut self, d: u32) -> String {
        if let Some(s) = self.reuse(&Ty::Char, d) {
            return s;
        }
        if d > 0 && self.chance(25) {
            let e = self.int_expr(&Ty::I32, d - 1);
            return format!("((65 + (({e}) & 15)) as char)");
        }
        let c = *self.rng.choose(CHARS);
        c.to_string()
    }

    fn str_expr(&mut self, d: u32) -> String {
        if let Some(s) = self.reuse(&Ty::Str, d) {
            return s;
        }
        if d == 0 {
            return self.word();
        }
        match self.rng.int(0, 8) {
            0 => format!("to_string({})", self.int_expr(&Ty::I32, d - 1)),
            1 => format!("i64_to_string({})", self.int_expr(&Ty::I64, d - 1)),
            2 => format!("f64_to_string({})", self.f64_expr(d - 1)),
            3 => format!("char_to_string({})", self.char_expr(d - 1)),
            4 | 5 => {
                // additive growth only: the right operand never aliases the target
                let l = self.str_expr(d - 1);
                let r = self.str_atom(d - 1);
                format!("({l} + {r})")
            }
            _ => self.word(),
        }
    }

    fn str_atom(&mut self, d: u32) -> String {
        match self.rng.int(0, 3) {
            0 if d > 0 => format!("to_string({})", self.int_expr(&Ty::I32, d - 1)),
            1 if d > 0 => format!("char_to_string({})", self.char_expr(d - 1)),
            _ => self.word(),
        }
    }

    fn bool_expr(&mut self, d: u32) -> String {
        if d == 0 {
            if self.chance(40) {
                if let Some(p) = self.leaf(&Ty::Bool) {
                    return p;
                }
            }
            return self.rng.bool().to_string();
        }
        if let Some(s) = self.reuse(&Ty::Bool, d) {
            return s;
        }
        match self.rng.int(0, 11) {
            0 | 1 => {
                let ty = if self.rng.bool() { Ty::I32 } else { Ty::I64 };
                let saved = self.no_big;
                self.no_big = true;
                let l = self.int_expr(&ty, d - 1);
                let r = self.int_expr(&ty, d - 1);
                self.no_big = saved;
                let op = *self.rng.choose(CMP);
                format!("({l}) {op} ({r})")
            }
            2 => {
                let l = self.f64_expr(d - 1);
                let r = self.f64_expr(d - 1);
                let op = *self.rng.choose(CMP);
                format!("({l}) {op} ({r})")
            }
            3 => {
                let l = self.char_expr(d - 1);
                let r = self.char_expr(d - 1);
                let op = *self.rng.choose(CMP);
                format!("({l}) {op} ({r})")
            }
            4 => {
                let l = self.str_expr(d - 1);
                let r = self.str_expr(d - 1);
                let op = if self.rng.bool() { "==" } else { "!=" };
                format!("({l}) {op} ({r})")
            }
            5 => {
                let l = self.bool_expr(d - 1);
                let r = self.bool_expr(d - 1);
                let op = if self.rng.bool() { "==" } else { "!=" };
                format!("(({l}) {op} ({r}))")
            }
            6 | 7 => self.agg_eq(d),
            8 => format!("!({})", self.bool_expr(d - 1)),
            _ => {
                let l = self.bool_expr(d - 1);
                let r = self.bool_expr(d - 1);
                let op = if self.rng.bool() { "&&" } else { "||" };
                format!("(({l}) {op} ({r}))")
            }
        }
    }

    /// `==` / `!=` on a tuple, struct or enum (never on arrays).
    fn agg_eq(&mut self, d: u32) -> String {
        let mut cands: Vec<Ty> = Vec::new();
        for i in 0..self.structs.len() {
            cands.push(Ty::Struct(i));
        }
        for i in 0..self.enums.len() {
            cands.push(Ty::Enum(i));
        }
        let tup = Ty::Tup(vec![self.scalar_ty(), self.scalar_ty()]);
        cands.push(tup);
        let ty = cands[self.pick(cands.len())].clone();
        if !self.eq_ok(&ty) {
            return self.rng.bool().to_string();
        }
        let saved = self.force;
        self.force = true;
        let l = self.expr(&ty, d - 1);
        let r = self.expr(&ty, d - 1);
        self.force = saved;
        let op = if self.rng.bool() { "==" } else { "!=" };
        format!("(({l}) {op} ({r}))")
    }

    fn agg_expr(&mut self, ty: &Ty, d: u32) -> String {
        if let Some(s) = self.reuse(ty, d) {
            return s;
        }
        let sub = d.saturating_sub(1);
        match ty {
            Ty::Tup(es) => {
                let parts: Vec<String> = es.iter().map(|e| self.expr(e, sub)).collect();
                format!("({})", parts.join(", "))
            }
            Ty::Struct(i) => {
                let fields = self.structs[*i].fields.clone();
                let mut parts: Vec<String> = Vec::new();
                for (n, t) in &fields {
                    let e = self.expr(t, sub);
                    parts.push(format!("{n}: {e}"));
                }
                for k in (1..parts.len()).rev() {
                    let j = self.rng.int(0, k as i32) as usize;
                    parts.swap(k, j);
                }
                format!("{} {{ {} }}", self.structs[*i].name, parts.join(", "))
            }
            Ty::Enum(i) => {
                let nv = self.enums[*i].variants.len();
                let v = self.pick(nv);
                self.enum_lit(*i, v, sub)
            }
            Ty::Arr(e, n) => {
                let saved = self.force;
                if self.has_i64(e) {
                    self.force = true;
                }
                let parts: Vec<String> = (0..*n).map(|_| self.expr(e, sub)).collect();
                self.force = saved;
                format!("[{}]", parts.join(", "))
            }
            _ => unreachable!(),
        }
    }

    fn enum_lit(&mut self, ei: usize, v: usize, d: u32) -> String {
        let en = self.enums[ei].name.clone();
        let (vn, payload) = self.enums[ei].variants[v].clone();
        if payload.is_empty() {
            format!("{en}::{vn}")
        } else {
            let args: Vec<String> = payload.iter().map(|p| self.expr(p, d)).collect();
            format!("{en}::{vn}({})", args.join(", "))
        }
    }

    fn match_expr(&mut self, ty: &Ty, d: u32) -> String {
        // `match` as an expression: arms are `Pattern => expr`, comma separated.
        let s = self.int_expr(&Ty::I32, d.saturating_sub(1));
        let a = self.expr(ty, d.saturating_sub(1));
        let b = self.expr(ty, d.saturating_sub(1));
        let c = self.expr(ty, d.saturating_sub(1));
        format!("(match ({s}) & 3 {{ 0 => {a}, 1 => {b}, _ => {c} }})")
    }

    // ----- value printing / checking -----

    fn print_value(&mut self, path: &str, ty: &Ty) {
        match ty {
            Ty::I32 => self.line(format!("print_i32({path});")),
            Ty::I64 => self.line(format!("print_i64({path});")),
            Ty::F64 => self.line(format!("print_f64({path});")),
            Ty::Bool => self.line(format!("print_bool({path});")),
            Ty::Char => self.line(format!("print_char({path});")),
            Ty::Str => self.line(format!("println({path});")),
            Ty::Tup(es) => {
                for (i, e) in es.iter().enumerate() {
                    self.print_value(&format!("{path}.{i}"), e);
                }
            }
            Ty::Struct(si) => {
                let fields = self.structs[*si].fields.clone();
                for (n, t) in &fields {
                    self.print_value(&format!("{path}.{n}"), t);
                }
            }
            Ty::Arr(e, n) => {
                for k in 0..*n {
                    self.print_value(&format!("{path}[{k}]"), e);
                }
            }
            Ty::Enum(ei) => self.print_enum(path, *ei),
        }
    }

    fn print_enum(&mut self, path: &str, ei: usize) {
        let en = self.enums[ei].name.clone();
        let variants = self.enums[ei].variants.clone();
        self.open(format!("match {path} {{"));
        let catch_last = self.chance(25);
        for (k, (vn, payload)) in variants.iter().enumerate() {
            if catch_last && k == variants.len() - 1 {
                self.line("_ => { println(\"other\"); }");
                continue;
            }
            let names: Vec<String> = payload.iter().map(|_| self.fresh("p")).collect();
            let pat = if payload.is_empty() {
                format!("{en}::{vn}")
            } else {
                format!("{en}::{vn}({})", names.join(", "))
            };
            self.open(format!("{pat} => {{"));
            self.line(format!("println(\"{vn}\");"));
            for (n, t) in names.iter().zip(payload.iter()) {
                self.print_value(n, t);
            }
            self.close();
        }
        self.close();
    }

    /// Boolean expression that is true when two scalar (or comparable enum)
    /// leaves hold the same value; NaN compares equal to anything.
    fn same_leaf(&self, a: &str, b: &str, ty: &Ty) -> String {
        match ty {
            Ty::F64 => format!("(!({a} < {b}) && !({a} > {b}))"),
            _ => format!("({a} == {b})"),
        }
    }

    /// Copy every scalar leaf (and comparable enum) reachable from `base`
    /// into fresh `let`s. Scalars cannot alias, so a later mismatch proves
    /// that a by-value aggregate was mutated through another name.
    fn take_snapshot(&mut self, base: &str, ty: &Ty) -> Vec<Snap> {
        let mut places = Vec::new();
        self.collect(String::new(), ty, true, &mut places);
        let mut out = Vec::new();
        for p in places {
            let leaf = !Self::is_agg(&p.ty) || matches!(p.ty, Ty::Enum(_));
            if !leaf || out.len() >= 10 {
                continue;
            }
            if matches!(p.ty, Ty::Enum(_)) && !self.eq_ok(&p.ty) {
                continue;
            }
            let var = self.fresh("sn");
            self.line(format!("let {var} = {base}{};", p.path));
            out.push(Snap {
                var,
                suffix: p.path,
                ty: p.ty,
            });
        }
        out
    }

    fn check_snapshot(&mut self, base: &str, snaps: &[Snap]) {
        if snaps.is_empty() {
            return;
        }
        let parts: Vec<String> = snaps
            .iter()
            .map(|s| self.same_leaf(&format!("{base}{}", s.suffix), &s.var, &s.ty))
            .collect();
        self.line(format!(
            "if (!({})) {{ println(\"VIOLATION\"); }}",
            parts.join(" && ")
        ));
    }

    // ----- scopes -----

    fn scope<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let mark = self.env.len();
        let r = f(self);
        self.env.truncate(mark);
        r
    }

    fn push_var(&mut self, name: &str, ty: Ty, assignable: bool) {
        self.env.push(Var {
            name: name.to_string(),
            ty,
            assignable,
        });
    }

    // ----- statements -----

    fn block_body(&mut self, n: i32) {
        self.depth += 1;
        for _ in 0..n {
            self.stmt();
        }
        self.depth -= 1;
    }

    fn stmt(&mut self) {
        let nested_ok = self.depth < 3;
        match self.rng.int(0, 27) {
            0..=3 => self.let_stmt(),
            4 => self.let_tuple_stmt(),
            5..=7 => self.assign_stmt(),
            8..=10 => self.compound_stmt(),
            11 | 12 => self.print_stmt(),
            13 | 14 if nested_ok => self.if_stmt(),
            15 | 16 if nested_ok => self.match_stmt(),
            17 if nested_ok => self.if_let_stmt(),
            18 | 19 if nested_ok => self.for_stmt(),
            20 if nested_ok => self.while_stmt(),
            21 => self.call_stmt(),
            22 => self.copy_check_stmt(),
            23 => {
                self.line("yield;");
            }
            24 => self.index_stmt(),
            25 if nested_ok => {
                self.open("{");
                self.scope(|g| g.block_body(2));
                self.close();
            }
            26 => self.big_stmt(),
            _ => self.print_stmt(),
        }
    }

    fn let_stmt(&mut self) {
        let ty = self.rand_ty(2);
        self.let_var(&ty, true);
    }

    /// `let [mut] vN[: T] = expr;` and returns the new variable's name.
    fn let_var(&mut self, ty: &Ty, allow_mut: bool) -> String {
        let name = self.fresh("v");
        let m = allow_mut && self.chance(70);
        let e = self.expr(ty, 2);
        let ann = self.has_i64(ty) || self.chance(25);
        let tn = self.ty_name(ty);
        let ann = if ann { format!(": {tn}") } else { String::new() };
        self.line(format!("let {}{name}{ann} = {e};", if m { "mut " } else { "" }));
        self.push_var(&name, ty.clone(), m);
        name
    }

    fn big_stmt(&mut self) {
        // literals that only fit i64, with an annotation giving the type
        let name = self.fresh("big");
        let lit = *self.rng.choose(&[
            "4294967296",
            "0xFFFF_FFFF",
            "0x7FFF_FFFF_FFFF_FFFF",
            "(-4294967296)",
            "9_223_372_036_854_775_807",
            "0b1_0000_0000_0000_0000_0000_0000_0000_0000",
        ]);
        self.line(format!("let mut {name}: i64 = {lit};"));
        self.push_var(&name, Ty::I64, true);
        self.line(format!("print_i64({name});"));
        let k = self.rng.int(1, 70);
        self.line(format!("print_i64({name} >> {k});"));
        self.line(format!("print_i64({name} * 3 + 1);"));
        self.line(format!("print_i32(({name}) as i32);"));
    }

    fn let_tuple_stmt(&mut self) {
        let n = self.rng.int(2, 3);
        let tys: Vec<Ty> = (0..n).map(|_| self.rand_ty(1)).collect();
        let ty = Ty::Tup(tys.clone());
        let saved = self.force;
        self.force = true;
        let e = self.expr(&ty, 2);
        self.force = saved;
        let m = self.chance(50);
        let mut names = Vec::new();
        let mut binds = Vec::new();
        for t in &tys {
            if self.chance(15) {
                names.push("_".to_string());
            } else {
                let nm = self.fresh("d");
                names.push(nm.clone());
                binds.push((nm, t.clone()));
            }
        }
        self.line(format!(
            "let {}({}) = {e};",
            if m { "mut " } else { "" },
            names.join(", ")
        ));
        for (nm, t) in binds {
            self.push_var(&nm, t, m);
        }
    }

    fn assign_target(&mut self, want_assignable: bool) -> Option<(String, Ty)> {
        let places: Vec<Place> = self
            .all_places()
            .into_iter()
            .filter(|p| p.assignable || !want_assignable)
            .collect();
        if places.is_empty() {
            return None;
        }
        let p = places[self.pick(places.len())].clone();
        // sometimes write through a dynamic index instead
        if let Ty::Arr(e, n) = &p.ty {
            if self.chance(50) {
                return Some((self.dyn_index(&p.path, *n), (**e).clone()));
            }
        }
        Some((p.path, p.ty))
    }

    fn assign_stmt(&mut self) {
        let Some((target, ty)) = self.assign_target(true) else {
            return self.let_stmt();
        };
        let saved = self.force;
        if self.has_i64(&ty) && matches!(ty, Ty::Arr(..)) {
            self.force = true;
        }
        let e = self.expr(&ty, 2);
        self.force = saved;
        self.line(format!("{target} = {e};"));
        if !Self::is_agg(&ty) {
            self.print_value(&target, &ty);
        }
    }

    fn compound_stmt(&mut self) {
        let places: Vec<Place> = self
            .all_places()
            .into_iter()
            .filter(|p| p.assignable && matches!(p.ty, Ty::I32 | Ty::I64 | Ty::F64 | Ty::Str))
            .collect();
        if places.is_empty() {
            return self.let_stmt();
        }
        let p = places[self.pick(places.len())].clone();
        let (target, ty) = (p.path.clone(), p.ty.clone());
        match ty {
            Ty::I32 | Ty::I64 => {
                let op = *self
                    .rng
                    .choose(&["+=", "-=", "*=", "&=", "|=", "^=", "<<=", ">>=", "/=", "%="]);
                let rhs = match op {
                    "/=" | "%=" => (*self.rng.choose(&["1", "2", "3", "7", "(-1)", "10"])).to_string(),
                    "<<=" | ">>=" if self.chance(70) => {
                        let k = *self.rng.choose(&[0, 1, 2, 5, 13, 31, 32, 33, 40, 63, 64]);
                        k.to_string()
                    }
                    _ => self.int_expr(&ty, 1),
                };
                self.line(format!("{target} {op} {rhs};"));
            }
            Ty::F64 => {
                let op = *self.rng.choose(&["+=", "-=", "*=", "/="]);
                let rhs = match op {
                    "*=" => format!("{}.0", self.rng.int(0, 3)),
                    "/=" => format!("{}.0", self.rng.int(1, 7)),
                    _ => self.f64_expr(1),
                };
                self.line(format!("{target} {op} {rhs};"));
            }
            _ => {
                let rhs = self.str_atom(1);
                self.line(format!("{target} += {rhs};"));
            }
        }
        // a compound assignment on an element with a dynamic index is
        // covered by `index_stmt`; print the written place
        self.print_value(&target, &ty);
    }

    fn index_stmt(&mut self) {
        // dynamic, masked index as an assignment / compound target
        let arrs: Vec<Place> = self
            .all_places()
            .into_iter()
            .filter(|p| p.assignable && matches!(&p.ty, Ty::Arr(e, _) if matches!(**e, Ty::I32 | Ty::I64)))
            .collect();
        if arrs.is_empty() {
            // a string indexed by a masked position
            let name = self.fresh("s");
            let w = *self.rng.choose(FOUR);
            self.line(format!("let {name} = \"{w}\";"));
            self.push_var(&name, Ty::Str, false);
            let saved = self.no_call;
            self.no_call = true;
            let e = self.int_expr(&Ty::I32, 1);
            self.no_call = saved;
            self.line(format!("print_char({name}[(({e}) & 3)]);"));
            self.line(format!("print_i32(len({name}));"));
            return;
        }
        let p = arrs[self.pick(arrs.len())].clone();
        let (el, n) = match &p.ty {
            Ty::Arr(e, n) => ((**e).clone(), *n),
            _ => unreachable!(),
        };
        let target = if self.chance(25) && !self.fns.is_empty() {
            // the index is evaluated twice by a compound assignment: a call
            // with side effects (prints) must run twice, as documented
            match self.call(&Ty::I32, 1) {
                Some(c) => format!("{}[(({c}) & {})]", p.path, n - 1),
                None => self.dyn_index(&p.path, n),
            }
        } else {
            self.dyn_index(&p.path, n)
        };
        match self.rng.int(0, 2) {
            0 => {
                let rhs = self.int_expr(&el, 1);
                self.line(format!("{target} = {rhs};"));
            }
            1 => {
                let op = *self.rng.choose(&["+=", "-=", "*=", "^=", "|=", "&=", "<<=", ">>="]);
                let rhs = self.int_expr(&el, 1);
                self.line(format!("{target} {op} {rhs};"));
            }
            _ => {
                let op = *self.rng.choose(&["+=", "^="]);
                let k = self.rng.int(0, 9);
                self.line(format!("{target} {op} {k};"));
            }
        }
        self.print_value(&p.path, &p.ty);
    }

    fn print_stmt(&mut self) {
        let ty = self.rand_ty(2);
        if !Self::is_agg(&ty) {
            let e = self.expr(&ty, 3);
            let (path, ty) = (format!("({e})"), ty);
            match ty {
                Ty::Str => self.line(format!("println{path};")),
                _ => self.print_value(&path, &ty),
            }
        } else {
            let name = self.let_var(&ty, false);
            self.print_value(&name, &ty);
        }
    }

    fn if_stmt(&mut self) {
        let c = self.bool_expr(2);
        self.open(format!("if ({c}) {{"));
        self.scope(|g| g.block_body(2));
        if self.chance(25) {
            self.else_open();
            // else-if chain
            let c2 = self.bool_expr(1);
            self.open(format!("if ({c2}) {{"));
            self.scope(|g| g.block_body(1));
            self.else_open();
            self.scope(|g| g.block_body(1));
            self.close();
            self.close();
        } else if self.chance(60) {
            self.else_open();
            self.scope(|g| g.block_body(2));
            self.close();
        } else {
            self.close();
        }
        if self.in_helper && self.chance(25) {
            self.early_return();
        }
    }

    fn early_return(&mut self) {
        let c = self.bool_expr(1);
        match self.ret_ty.clone() {
            Some(t) => {
                let e = self.expr(&t, 1);
                self.line(format!("if ({c}) {{ return {e}; }}"));
            }
            None => self.line(format!("if ({c}) {{ return; }}")),
        }
    }

    /// Random pattern for a variant: bindings (immutable) or `_` per slot.
    fn variant_pattern(&mut self, ei: usize, v: usize) -> (String, Vec<(String, Ty)>) {
        let en = self.enums[ei].name.clone();
        let (vn, payload) = self.enums[ei].variants[v].clone();
        if payload.is_empty() {
            return (format!("{en}::{vn}"), Vec::new());
        }
        let mut names = Vec::new();
        let mut binds = Vec::new();
        for t in &payload {
            if self.chance(25) {
                names.push("_".to_string());
            } else {
                let nm = self.fresh("b");
                names.push(nm.clone());
                binds.push((nm, t.clone()));
            }
        }
        (format!("{en}::{vn}({})", names.join(", ")), binds)
    }

    fn enum_scrutinee(&mut self, ei: usize) -> String {
        let ty = Ty::Enum(ei);
        let e = self.expr(&ty, 2);
        if e.contains('{') || e.contains("::") {
            // struct / enum literals: keep the scrutinee unambiguous
            let name = self.fresh("sc");
            self.line(format!("let {name} = {e};"));
            self.push_var(&name, ty, false);
            name
        } else {
            e
        }
    }

    fn match_stmt(&mut self) {
        if self.chance(65) {
            let ei = self.pick(self.enums.len());
            let scrut = self.enum_scrutinee(ei);
            self.open(format!("match {scrut} {{"));
            let nv = self.enums[ei].variants.len();
            let mut order: Vec<usize> = (0..nv).collect();
            for k in (1..nv).rev() {
                let j = self.rng.int(0, k as i32) as usize;
                order.swap(k, j);
            }
            let tail = if self.chance(30) { 1 } else { 0 };
            let tail_kind = self.rng.int(0, 1);
            let upto = nv - tail.min(nv);
            for &v in &order[..upto] {
                let (pat, binds) = self.variant_pattern(ei, v);
                self.arm(&pat, binds, true);
            }
            if tail == 1 || upto == 0 {
                if tail_kind == 0 {
                    self.arm("_", Vec::new(), false);
                } else {
                    let nm = self.fresh("w");
                    self.arm(&nm.clone(), vec![(nm, Ty::Enum(ei))], false);
                }
            }
            self.close();
        } else {
            let ty = match self.rng.int(0, 4) {
                0 | 1 => Ty::I32,
                2 => Ty::I64,
                3 => Ty::Char,
                _ => Ty::Str,
            };
            let ty = if self.chance(15) { Ty::Bool } else { ty };
            let scrut = {
                // a scrutinee has no expected type: no literal beyond i32
                // (and an `i64` binder must really be `i64`: cast from `i32`)
                let (saved, sforce) = (self.no_big, self.force);
                self.no_big = true;
                self.force = true;
                let e = self.expr(&ty, 2);
                self.no_big = saved;
                self.force = sforce;
                format!("({e})")
            };
            self.open(format!("match {scrut} {{"));
            let n = self.rng.int(1, 3);
            let mut used: Vec<String> = Vec::new();
            for _ in 0..n {
                let pat = self.literal_pattern(&ty);
                if used.contains(&pat) {
                    continue;
                }
                used.push(pat.clone());
                self.arm(&pat, Vec::new(), false);
            }
            if self.rng.bool() {
                self.arm("_", Vec::new(), false);
            } else {
                let nm = self.fresh("w");
                self.arm(&nm.clone(), vec![(nm, ty.clone())], false);
            }
            self.close();
        }
    }

    fn literal_pattern(&mut self, ty: &Ty) -> String {
        match ty {
            Ty::I32 | Ty::I64 => {
                let v = self.rng.int(-3, 12);
                if self.chance(15) {
                    format!("0x{:X}", v.max(0))
                } else {
                    v.to_string()
                }
            }
            Ty::Bool => self.rng.bool().to_string(),
            Ty::Char => (*self.rng.choose(CHARS)).to_string(),
            _ => self.word(),
        }
    }

    fn arm(&mut self, pat: &str, binds: Vec<(String, Ty)>, _enum_arm: bool) {
        let comma = if self.chance(30) { "," } else { "" };
        self.open(format!("{pat} => {{"));
        self.scope(|g| {
            for (n, t) in binds {
                g.push_var(&n, t, false);
            }
            let n = g.rng.int(1, 2);
            g.block_body(n);
        });
        self.indent -= 1;
        self.line(format!("}}{comma}"));
    }

    fn if_let_stmt(&mut self) {
        if self.chance(75) {
            let ei = self.pick(self.enums.len());
            let scrut = self.enum_scrutinee(ei);
            let nv = self.enums[ei].variants.len();
            let v = self.pick(nv);
            let (pat, binds) = if self.chance(10) {
                let nm = self.fresh("w");
                (nm.clone(), vec![(nm, Ty::Enum(ei))])
            } else {
                self.variant_pattern(ei, v)
            };
            self.open(format!("if let {pat} = {scrut} {{"));
            self.scope(|g| {
                for (n, t) in binds {
                    g.push_var(&n, t, false);
                }
                g.block_body(2);
            });
            if self.chance(55) {
                self.else_open();
                self.scope(|g| g.block_body(2));
            }
            self.close();
        } else {
            let ty = if self.rng.bool() { Ty::I32 } else { Ty::Char };
            let pat = self.literal_pattern(&ty);
            let e = self.expr(&ty, 2);
            self.open(format!("if let {pat} = ({e}) {{"));
            self.scope(|g| g.block_body(2));
            if self.chance(55) {
                self.else_open();
                self.scope(|g| g.block_body(1));
            }
            self.close();
        }
    }

    fn loop_extras(&mut self, var: Option<&str>) {
        // `break` / `continue` under a guard, `yield;`
        if self.chance(25) {
            let kw = if self.rng.bool() { "break" } else { "continue" };
            let k = self.rng.int(1, 3);
            let c = match var {
                Some(v) => format!("{v} == {k}"),
                None => self.bool_expr(1),
            };
            self.line(format!("if ({c}) {{ {kw}; }}"));
        }
        if self.chance(25) {
            self.line("yield;");
        }
    }

    fn for_stmt(&mut self) {
        if self.loop_depth >= if self.in_helper { 1 } else { 2 } {
            return self.let_stmt();
        }
        let lo = self.rng.int(0, 2);
        let hi = lo + self.rng.int(1, 4);
        let var = self.fresh("i");
        self.open(format!("for {var} in {lo}..{hi} {{"));
        let (sl, sin) = (self.loop_depth, self.in_loop);
        self.loop_depth += 1;
        self.in_loop = true;
        self.scope(|g| {
            g.push_var(&var, Ty::I32, false);
            let n = g.rng.int(1, 3);
            g.loop_extras(Some(var.as_str()));
            g.block_body(n);
        });
        self.loop_depth = sl;
        self.in_loop = sin;
        self.close();
    }

    fn while_stmt(&mut self) {
        if self.loop_depth >= if self.in_helper { 1 } else { 2 } {
            return self.let_stmt();
        }
        let w = self.fresh("w");
        let lim = self.rng.int(1, 4);
        self.line(format!("let mut {w} = 0;"));
        self.open(format!("while ({w} < {lim}) {{"));
        self.line(format!("{w} += 1;"));
        let (sl, sin) = (self.loop_depth, self.in_loop);
        self.loop_depth += 1;
        self.in_loop = true;
        self.scope(|g| {
            g.push_var(&w, Ty::I32, false);
            g.loop_extras(Some(w.as_str()));
            let n = g.rng.int(1, 3);
            g.block_body(n);
        });
        self.loop_depth = sl;
        self.in_loop = sin;
        self.close();
        self.line(format!("print_i32({w});"));
    }

    fn call_stmt(&mut self) {
        if self.fns.is_empty() || self.no_call {
            return self.print_stmt();
        }
        // prefer a helper whose first parameter is an aggregate we hold
        let k = self.pick(self.fns.len());
        let sig_params = self.fns[k].params.clone();
        let ret = self.fns[k].ret.clone();
        let snap = match sig_params.first() {
            Some(t) if Self::is_agg(t) => self
                .all_places()
                .into_iter()
                .find(|p| p.ty == *t && !p.path.contains('['))
                .map(|p| (p.path, t.clone())),
            _ => None,
        };
        let mut args: Vec<String> = Vec::new();
        for (i, p) in sig_params.iter().enumerate() {
            if i == 0 {
                if let Some((path, _)) = &snap {
                    args.push(path.clone());
                    continue;
                }
            }
            let a = self.expr(p, 1);
            args.push(a);
        }
        let call = format!("{}({})", self.fns[k].name, args.join(", "));
        let snap_name = match &snap {
            Some((path, t)) => Some((self.take_snapshot(path, t), path.clone())),
            None => None,
        };
        match ret {
            Some(t) => {
                let r = self.fresh("r");
                let ann = if self.has_i64(&t) {
                    format!(": {}", self.ty_name(&t))
                } else {
                    String::new()
                };
                self.line(format!("let {r}{ann} = {call};"));
                self.push_var(&r, t.clone(), false);
                self.print_value(&r, &t);
            }
            None => self.line(format!("{call};")),
        }
        if let Some((snaps, path)) = snap_name {
            self.check_snapshot(&path, &snaps);
        }
    }

    /// Copy semantics: copy an aggregate, mutate one side, the other must
    /// stay equal to a snapshot taken before.
    fn copy_check_stmt(&mut self) {
        let vars: Vec<(String, Ty, bool)> = self
            .env
            .iter()
            .filter(|v| Self::is_agg(&v.ty))
            .map(|v| (v.name.clone(), v.ty.clone(), v.assignable))
            .collect();
        if vars.is_empty() {
            return self.let_stmt();
        }
        let (name, ty, assignable) = vars[self.pick(vars.len())].clone();
        let snaps = self.take_snapshot(&name, &ty);
        let cp = self.fresh("cp");
        self.line(format!("let mut {cp} = {name};"));
        self.push_var(&cp, ty.clone(), true);
        let mutate_orig = assignable && self.rng.bool();
        let victim = if mutate_orig { name.clone() } else { cp.clone() };
        let n = self.rng.int(1, 3);
        for _ in 0..n {
            let places: Vec<Place> = self
                .var_places(&victim)
                .into_iter()
                .filter(|p| p.assignable && !Self::is_agg(&p.ty))
                .collect();
            if places.is_empty() {
                break;
            }
            let p = places[self.pick(places.len())].clone();
            let e = self.expr(&p.ty, 1);
            self.line(format!("{} = {e};", p.path));
        }
        // the untouched side must still equal the snapshot
        let other = if mutate_orig { cp.clone() } else { name.clone() };
        self.check_snapshot(&other, &snaps);
        self.print_value(&victim, &ty);
    }

    // ----- items -----

    fn build(&mut self) -> (Vec<String>, String) {
        let mut items = Vec::new();
        let n = self.rng.int(2, 5);
        for _ in 0..n {
            if self.rng.bool() {
                items.push(self.gen_struct());
            } else {
                items.push(self.gen_enum());
            }
        }
        if self.enums.is_empty() {
            items.push(self.gen_enum());
        }
        // bounded recursion: the argument is masked at every level
        if self.chance(50) {
            items.push(
                "fn fibr(n: i32) -> i32 {\n    let m = n & 7;\n    if (m < 2) { return m; }\n    return fibr(m - 1) + fibr(m - 2);\n}"
                    .to_string(),
            );
            self.fns.push(FnSig {
                name: "fibr".into(),
                params: vec![Ty::I32],
                ret: Some(Ty::I32),
            });
        }
        if self.chance(50) {
            items.push(
                "fn sumr(n: i32, acc: i64) -> i64 {\n    let m = n & 15;\n    if (m == 0) { return acc; }\n    return sumr(m - 1, acc + (m as i64));\n}"
                    .to_string(),
            );
            self.fns.push(FnSig {
                name: "sumr".into(),
                params: vec![Ty::I32, Ty::I64],
                ret: Some(Ty::I64),
            });
        }
        if self.chance(40) {
            // mutual recursion
            items.push(
                "fn is_even(n: i32) -> bool {\n    let m = n & 15;\n    if (m == 0) { return true; }\n    return is_odd(m - 1);\n}\nfn is_odd(n: i32) -> bool {\n    let m = n & 15;\n    if (m == 0) { return false; }\n    return is_even(m - 1);\n}"
                    .to_string(),
            );
            self.fns.push(FnSig {
                name: "is_even".into(),
                params: vec![Ty::I32],
                ret: Some(Ty::Bool),
            });
        }
        let nf = self.rng.int(3, 6);
        for k in 0..nf {
            items.push(self.helper(k as usize));
        }
        let main = self.main_fn();
        (items, main)
    }

    fn gen_struct(&mut self) -> String {
        let idx = self.structs.len();
        let nf = self.rng.int(1, 4);
        let mut fields = Vec::new();
        for k in 0..nf {
            let t = self.rand_ty(1);
            fields.push((format!("f{k}"), t));
        }
        let text = format!(
            "struct S{idx} {{ {} }}",
            fields
                .iter()
                .map(|(n, t)| format!("{n}: {}", self.ty_name(t)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        self.structs.push(StructDef {
            name: format!("S{idx}"),
            fields,
        });
        text
    }

    fn gen_enum(&mut self) -> String {
        let idx = self.enums.len();
        let nv = self.rng.int(1, 4);
        let mut variants = Vec::new();
        for k in 0..nv {
            let np = self.rng.int(0, 3);
            let mut payload = Vec::new();
            for _ in 0..np {
                if !self.structs.is_empty() && self.chance(20) {
                    payload.push(Ty::Struct(self.pick(self.structs.len())));
                } else {
                    payload.push(self.scalar_ty());
                }
            }
            variants.push((format!("V{k}"), payload));
        }
        let text = format!(
            "enum E{idx} {{ {} }}",
            variants
                .iter()
                .map(|(n, ps)| {
                    if ps.is_empty() {
                        n.clone()
                    } else {
                        format!(
                            "{n}({})",
                            ps.iter().map(|p| self.ty_name(p)).collect::<Vec<_>>().join(", ")
                        )
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
        self.enums.push(EnumDef {
            name: format!("E{idx}"),
            variants,
        });
        text
    }

    fn helper(&mut self, k: usize) -> String {
        let nparams = self.rng.int(0, 3);
        let params: Vec<Ty> = (0..nparams).map(|_| self.rand_ty(2)).collect();
        let ret = if self.chance(20) { None } else { Some(self.rand_ty(2)) };
        let name = format!("h{k}");
        self.env.clear();
        self.in_helper = true;
        self.ret_ty = ret.clone();
        self.depth = 0;
        self.loop_depth = 0;
        self.force = false;
        self.no_big = false;
        self.no_call = false;
        let mut pnames = Vec::new();
        for (i, t) in params.iter().enumerate() {
            let pn = format!("p{i}");
            self.push_var(&pn, t.clone(), false);
            pnames.push(pn);
        }
        let sig = params
            .iter()
            .zip(pnames.iter())
            .map(|(t, n)| format!("{n}: {}", self.ty_name(t)))
            .collect::<Vec<_>>()
            .join(", ");
        let rt = match &ret {
            Some(t) => self.ty_name(t),
            None => "()".into(),
        };
        self.open(format!("fn {name}({sig}) -> {rt} {{"));
        // by-value aggregates: mutate a copy, print the original afterwards
        let mut originals = Vec::new();
        for (pn, t) in pnames.iter().zip(params.iter()) {
            if Self::is_agg(t) {
                let c = format!("c_{pn}");
                self.line(format!("let mut {c} = {pn};"));
                self.push_var(&c, t.clone(), true);
                let snaps = self.take_snapshot(pn, t);
                originals.push((pn.clone(), snaps, t.clone()));
            }
        }
        let n = self.rng.int(2, 5);
        self.block_body(n);
        for (pn, snaps, _) in &originals {
            self.check_snapshot(pn, snaps);
        }
        for (pn, _, t) in &originals {
            self.print_value(pn, t);
        }
        if let Some(t) = &ret {
            let e = self.expr(t, 2);
            if self.rng.bool() {
                self.line(format!("return {e};"));
            } else {
                self.line(e);
            }
        } else if self.chance(30) {
            self.line("return;");
        }
        self.close();
        let text = self.take_out();
        self.fns.push(FnSig {
            name,
            params,
            ret,
        });
        text.trim_end().to_string()
    }

    fn main_fn(&mut self) -> String {
        self.env.clear();
        self.in_helper = false;
        self.ret_ty = None;
        self.depth = 0;
        self.loop_depth = 0;
        self.force = false;
        self.no_big = false;
        self.no_call = false;
        self.open("fn main() -> i32 {");
        // seed the environment with one variable per interesting type
        let seeds = [
            Ty::I32,
            Ty::I64,
            Ty::F64,
            Ty::Str,
            Ty::Tup(vec![Ty::I32, Ty::Tup(vec![Ty::I64, Ty::Bool])]),
        ];
        for t in seeds.iter() {
            self.let_var(t, true);
        }
        for e in 0..self.enums.len() {
            self.let_var(&Ty::Enum(e), true);
        }
        for s in 0..self.structs.len().min(2) {
            self.let_var(&Ty::Struct(s), true);
        }
        let n = self.rng.int(12, 24);
        for _ in 0..n {
            self.stmt();
        }
        // final state of every top-level variable
        let vars: Vec<(String, Ty)> = self
            .env
            .iter()
            .map(|v| (v.name.clone(), v.ty.clone()))
            .collect();
        for (n, t) in vars {
            self.print_value(&n, &t);
        }
        let e = self.int_expr(&Ty::I32, 2);
        self.line(format!("return {e};"));
        self.close();
        self.take_out().trim_end().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_program_is_deterministic_and_varies() {
        let a = gen_lang_source(&mut FuzzRng::new(5));
        let b = gen_lang_source(&mut FuzzRng::new(5));
        let c = gen_lang_source(&mut FuzzRng::new(6));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.contains("fn main() -> i32"));
        assert!(a.contains("enum E0"));
    }

    /// `AETHER_LANG_SEED=N cargo test dump_lang_sample -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_lang_sample() {
        let seed = std::env::var("AETHER_LANG_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1u64);
        let p = gen_lang_program(&mut FuzzRng::new(seed));
        if let Ok(dir) = std::env::var("AETHER_LANG_DIR") {
            for (n, t) in &p.files {
                let path = std::path::Path::new(&dir).join(n);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, t).unwrap();
            }
            return;
        }
        for (n, t) in &p.files {
            println!("// ==== {n} ====\n{t}");
        }
    }

    #[test]
    fn layouts_include_multi_file_programs() {
        let mut multi = 0;
        for s in 0..60u64 {
            let p = gen_lang_program(&mut FuzzRng::new(s));
            if !p.is_single() {
                multi += 1;
                assert!(p.files[0].1.contains("use \""), "{}", p.files[0].1);
            }
        }
        assert!(multi > 5, "only {multi} multi-file programs of 60");
    }
}

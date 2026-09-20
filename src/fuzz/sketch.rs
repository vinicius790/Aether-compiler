//! Typed program sketch: the generator's real IR.
//!
//! Source text is only a pretty-printer. Aspect-preserving mutation walks
//! this tree and rewrites nodes while keeping four *aspects*:
//!
//! * **type** — every expression keeps its `STy`;
//! * **name** — no use of an unbound name, no assign to a loop index;
//! * **termination** — `for` bounds stay small and non-empty-or-empty but finite;
//! * **arity** — helpers stay `i32 -> i32`, calls only target existing helpers.
//!
//! A stricter subset also keeps the **value** aspect (`e` → `e+0`, `if true`),
//! so the differential oracle can run on the mutant.

use super::rng::FuzzRng;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum STy {
    I32,
    Bool,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    LitI32(i32),
    LitBool(bool),
    Var(String),
    Bin {
        op: &'static str,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Un {
        op: &'static str,
        inner: Box<Expr>,
    },
    Call {
        name: String,
        arg: Box<Expr>,
    },
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub ty: STy,
    pub kind: ExprKind,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    LetMut {
        name: String,
        init: Expr,
    },
    Assign {
        name: String,
        value: Expr,
    },
    If {
        cond: Expr,
        then_s: Vec<Stmt>,
        else_s: Option<Vec<Stmt>>,
    },
    For {
        var: String,
        lo: i32,
        hi: i32,
        body: Vec<Stmt>,
    },
    Print(Expr),
    Break,
}

#[derive(Debug, Clone)]
pub struct FnSketch {
    pub name: String,
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
    pub ret: Expr,
}

#[derive(Debug, Clone)]
pub struct ProgramSketch {
    pub helpers: Vec<FnSketch>,
    pub main: FnSketch,
}

#[derive(Debug, Clone)]
struct Bind {
    name: String,
    ty: STy,
    mutable: bool,
    loop_index: bool,
}

pub fn gen_sketch(rng: &mut FuzzRng) -> ProgramSketch {
    let n_helpers = rng.int(0, 2) as usize;
    let helpers: Vec<FnSketch> = (0..n_helpers).map(|i| gen_helper(rng, i)).collect();
    let main = gen_main(rng, n_helpers);
    ProgramSketch { helpers, main }
}

pub fn gen_program(rng: &mut FuzzRng) -> String {
    gen_sketch(rng).render()
}

impl ProgramSketch {
    pub fn render(&self) -> String {
        let mut s = String::new();
        for h in &self.helpers {
            s.push_str(&h.render());
            s.push('\n');
        }
        s.push_str(&self.main.render());
        s
    }

    pub fn helper_count(&self) -> usize {
        self.helpers.len()
    }
}

impl FnSketch {
    fn render(&self) -> String {
        let params = if self.name == "main" {
            String::new()
        } else {
            self.params
                .iter()
                .map(|p| format!("{p}: i32"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut body = String::new();
        for st in &self.body {
            body.push_str(&st.render(1));
        }
        body.push_str(&format!("    return {};\n", self.ret.render()));
        format!("fn {}({params}) -> i32 {{\n{body}}}\n", self.name)
    }
}

impl Stmt {
    fn render(&self, indent: usize) -> String {
        let p = "    ".repeat(indent);
        match self {
            Stmt::LetMut { name, init } => format!("{p}let mut {name} = {};\n", init.render()),
            Stmt::Assign { name, value } => format!("{p}{name} = {};\n", value.render()),
            Stmt::If {
                cond,
                then_s,
                else_s,
            } => {
                let mut s = format!("{p}if {} {{\n", cond.render());
                for st in then_s {
                    s.push_str(&st.render(indent + 1));
                }
                s.push_str(&format!("{p}}}"));
                if let Some(e) = else_s {
                    s.push_str(" else {\n");
                    for st in e {
                        s.push_str(&st.render(indent + 1));
                    }
                    s.push_str(&format!("{p}}}"));
                }
                s.push('\n');
                s
            }
            Stmt::For { var, lo, hi, body } => {
                let mut s = format!("{p}for {var} in {lo}..{hi} {{\n");
                for st in body {
                    s.push_str(&st.render(indent + 1));
                }
                s.push_str(&format!("{p}}}\n"));
                s
            }
            Stmt::Print(e) => format!("{p}print_i32({});\n", e.render()),
            Stmt::Break => format!("{p}break;\n"),
        }
    }
}

impl Expr {
    fn render(&self) -> String {
        match &self.kind {
            ExprKind::LitI32(n) => n.to_string(),
            ExprKind::LitBool(b) => b.to_string(),
            ExprKind::Var(n) => n.clone(),
            ExprKind::Bin { op, lhs, rhs } => {
                format!("({} {op} {})", lhs.render(), rhs.render())
            }
            ExprKind::Un { op, inner } => format!("({op}{})", inner.render()),
            ExprKind::Call { name, arg } => format!("{name}({})", arg.render()),
        }
    }
}

fn gen_helper(rng: &mut FuzzRng, idx: usize) -> FnSketch {
    let mut env = vec![Bind {
        name: "p0".into(),
        ty: STy::I32,
        mutable: false,
        loop_index: false,
    }];
    let body = gen_stmts(rng, &mut env, 1, idx, false);
    let ret = gen_expr(rng, &env, idx, STy::I32, 2);
    FnSketch {
        name: format!("h{idx}"),
        params: vec!["p0".into()],
        body,
        ret,
    }
}

fn gen_main(rng: &mut FuzzRng, helpers: usize) -> FnSketch {
    let mut env = vec![Bind {
        name: "acc".into(),
        ty: STy::I32,
        mutable: true,
        loop_index: false,
    }];
    let mut body = vec![Stmt::LetMut {
        name: "acc".into(),
        init: lit_i32(0),
    }];
    let n = rng.int(1, 4) as usize;
    for _ in 0..n {
        body.extend(gen_stmts(rng, &mut env, 1, helpers, false));
    }
    FnSketch {
        name: "main".into(),
        params: Vec::new(),
        body,
        ret: Expr {
            ty: STy::I32,
            kind: ExprKind::Var("acc".into()),
        },
    }
}

fn gen_stmts(
    rng: &mut FuzzRng,
    env: &mut Vec<Bind>,
    n_target: usize,
    helpers: usize,
    in_loop: bool,
) -> Vec<Stmt> {
    let mut out = Vec::new();
    let n = rng.int(1, n_target as i32 + 1) as usize;
    for _ in 0..n {
        out.push(gen_stmt(rng, env, helpers, in_loop));
    }
    out
}

fn gen_stmt(rng: &mut FuzzRng, env: &mut Vec<Bind>, helpers: usize, in_loop: bool) -> Stmt {
    match rng.int(0, if in_loop { 7 } else { 6 }) {
        0 => {
            let name = format!("v{}", env.len());
            let init = gen_expr(rng, env, helpers, STy::I32, 2);
            env.push(Bind {
                name: name.clone(),
                ty: STy::I32,
                mutable: true,
                loop_index: false,
            });
            Stmt::LetMut { name, init }
        }
        1 => {
            if let Some(v) = pick_mutable(rng, env, STy::I32) {
                Stmt::Assign {
                    name: v,
                    value: gen_expr(rng, env, helpers, STy::I32, 2),
                }
            } else {
                let name = format!("v{}", env.len());
                let init = gen_expr(rng, env, helpers, STy::I32, 1);
                env.push(Bind {
                    name: name.clone(),
                    ty: STy::I32,
                    mutable: true,
                    loop_index: false,
                });
                Stmt::LetMut { name, init }
            }
        }
        2 => {
            let cond = gen_expr(rng, env, helpers, STy::Bool, 2);
            let mut then_env = env.clone();
            let then_s = gen_stmts(rng, &mut then_env, 1, helpers, in_loop);
            let else_s = if rng.bool() {
                let mut else_env = env.clone();
                Some(gen_stmts(rng, &mut else_env, 1, helpers, in_loop))
            } else {
                None
            };
            Stmt::If {
                cond,
                then_s,
                else_s,
            }
        }
        3 if !in_loop => {
            let lo = rng.int(0, 2);
            let hi = lo + rng.int(0, 3);
            let var = format!("i{}", env.len());
            let mut loop_env = env.clone();
            loop_env.push(Bind {
                name: var.clone(),
                ty: STy::I32,
                mutable: true,
                loop_index: true,
            });
            let body = vec![gen_stmt(rng, &mut loop_env, helpers, true)];
            Stmt::For {
                var,
                lo,
                hi,
                body,
            }
        }
        4 => Stmt::Print(gen_expr(rng, env, helpers, STy::I32, 2)),
        5 if helpers > 0 => {
            let call = call_helper(rng, env, helpers);
            if has_name(env, "acc") {
                Stmt::Assign {
                    name: "acc".into(),
                    value: call,
                }
            } else {
                let name = format!("v{}", env.len());
                env.push(Bind {
                    name: name.clone(),
                    ty: STy::I32,
                    mutable: true,
                    loop_index: false,
                });
                Stmt::LetMut { name, init: call }
            }
        }
        6 if in_loop && rng.bool() => Stmt::Break,
        _ => {
            if has_name(env, "acc") {
                Stmt::Assign {
                    name: "acc".into(),
                    value: Expr {
                        ty: STy::I32,
                        kind: ExprKind::Bin {
                            op: "+",
                            lhs: Box::new(var_i32("acc")),
                            rhs: Box::new(gen_expr(rng, env, helpers, STy::I32, 1)),
                        },
                    },
                }
            } else {
                Stmt::Print(gen_expr(rng, env, helpers, STy::I32, 1))
            }
        }
    }
}

fn gen_expr(rng: &mut FuzzRng, env: &[Bind], helpers: usize, ty: STy, depth: i32) -> Expr {
    if depth <= 0 {
        return leaf(rng, env, ty);
    }
    match ty {
        STy::I32 => match rng.int(0, 7) {
            0 | 1 => leaf(rng, env, STy::I32),
            2 => bin(
                *rng.choose(&["+", "-", "*"]),
                STy::I32,
                gen_expr(rng, env, helpers, STy::I32, depth - 1),
                gen_expr(rng, env, helpers, STy::I32, depth - 1),
            ),
            3 => bin(
                "/",
                STy::I32,
                gen_expr(rng, env, helpers, STy::I32, depth - 1),
                lit_i32(rng.int(1, 7)),
            ),
            4 => bin(
                "%",
                STy::I32,
                gen_expr(rng, env, helpers, STy::I32, depth - 1),
                lit_i32(rng.int(1, 7)),
            ),
            5 => Expr {
                ty: STy::I32,
                kind: ExprKind::Un {
                    op: "-",
                    inner: Box::new(gen_expr(rng, env, helpers, STy::I32, depth - 1)),
                },
            },
            6 if helpers > 0 => call_helper(rng, env, helpers),
            _ => leaf(rng, env, STy::I32),
        },
        STy::Bool => match rng.int(0, 5) {
            0 => leaf(rng, env, STy::Bool),
            1 => bin(
                *rng.choose(&["<", ">", "<=", ">=", "==", "!="]),
                STy::Bool,
                gen_expr(rng, env, helpers, STy::I32, 0),
                gen_expr(rng, env, helpers, STy::I32, 0),
            ),
            2 => bin(
                "&&",
                STy::Bool,
                gen_expr(rng, env, helpers, STy::Bool, depth - 1),
                gen_expr(rng, env, helpers, STy::Bool, depth - 1),
            ),
            3 => bin(
                "||",
                STy::Bool,
                gen_expr(rng, env, helpers, STy::Bool, depth - 1),
                gen_expr(rng, env, helpers, STy::Bool, depth - 1),
            ),
            _ => Expr {
                ty: STy::Bool,
                kind: ExprKind::Un {
                    op: "!",
                    inner: Box::new(gen_expr(rng, env, helpers, STy::Bool, depth - 1)),
                },
            },
        },
    }
}

fn leaf(rng: &mut FuzzRng, env: &[Bind], ty: STy) -> Expr {
    let vars: Vec<&str> = env
        .iter()
        .filter(|b| b.ty == ty)
        .map(|b| b.name.as_str())
        .collect();
    if !vars.is_empty() && rng.bool() {
        Expr {
            ty,
            kind: ExprKind::Var(vars[rng.int(0, (vars.len() - 1) as i32) as usize].to_string()),
        }
    } else {
        match ty {
            STy::I32 => lit_i32(rng.int(0, 12)),
            STy::Bool => lit_bool(rng.bool()),
        }
    }
}

fn call_helper(rng: &mut FuzzRng, env: &[Bind], helpers: usize) -> Expr {
    let idx = rng.int(0, helpers.saturating_sub(1) as i32) as usize;
    Expr {
        ty: STy::I32,
        kind: ExprKind::Call {
            name: format!("h{idx}"),
            arg: Box::new(leaf(rng, env, STy::I32)),
        },
    }
}

fn lit_i32(n: i32) -> Expr {
    Expr {
        ty: STy::I32,
        kind: ExprKind::LitI32(n),
    }
}

fn lit_bool(b: bool) -> Expr {
    Expr {
        ty: STy::Bool,
        kind: ExprKind::LitBool(b),
    }
}

fn var_i32(name: &str) -> Expr {
    Expr {
        ty: STy::I32,
        kind: ExprKind::Var(name.into()),
    }
}

fn bin(op: &'static str, ty: STy, lhs: Expr, rhs: Expr) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Bin {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
    }
}

fn has_name(env: &[Bind], name: &str) -> bool {
    env.iter().any(|b| b.name == name)
}

fn pick_mutable(rng: &mut FuzzRng, env: &[Bind], ty: STy) -> Option<String> {
    let muts: Vec<&str> = env
        .iter()
        .filter(|b| b.mutable && !b.loop_index && b.ty == ty)
        .map(|b| b.name.as_str())
        .collect();
    if muts.is_empty() {
        None
    } else {
        Some(muts[rng.int(0, (muts.len() - 1) as i32) as usize].to_string())
    }
}

/// Aspect-preserving mutation. `preserve_value` restricts the operator set
/// to identities and dead inserts so `main`'s result (and usually stdout)
/// stay the same.
pub fn mutate_aspect(rng: &mut FuzzRng, sk: &ProgramSketch, preserve_value: bool) -> ProgramSketch {
    let mut out = sk.clone();
    let n = rng.int(1, 3) as usize;
    for _ in 0..n {
        if preserve_value {
            apply_value_op(rng, &mut out);
        } else {
            apply_typed_op(rng, &mut out);
        }
    }
    out
}

pub fn crossover_sketch(rng: &mut FuzzRng, a: &ProgramSketch, b: &ProgramSketch) -> ProgramSketch {
    let mut out = a.clone();
    if b.helpers.is_empty() {
        return out;
    }
    let mut donor = b.helpers[rng.int(0, (b.helpers.len() - 1) as i32) as usize].clone();
    let idx = out.helpers.len();
    donor.name = format!("h{idx}");
    out.helpers.push(donor);
    out
}

fn apply_value_op(rng: &mut FuzzRng, sk: &mut ProgramSketch) {
    match rng.int(0, 3) {
        0 => wrap_identity_in_fn(rng, sk),
        1 => insert_dead_let(rng, sk),
        2 => wrap_body_if_true(rng, sk),
        _ => wrap_identity_in_fn(rng, sk),
    }
}

fn apply_typed_op(rng: &mut FuzzRng, sk: &mut ProgramSketch) {
    match rng.int(0, 6) {
        0 => tweak_literals(rng, sk),
        1 => swap_same_sort_binop(rng, sk),
        2 => insert_dead_let(rng, sk),
        3 => wrap_body_if_true(rng, sk),
        4 => tweak_for_bounds(rng, sk),
        5 => retarget_call(rng, sk),
        _ => wrap_identity_in_fn(rng, sk),
    }
}

fn wrap_identity_in_fn(rng: &mut FuzzRng, sk: &mut ProgramSketch) {
    let target = pick_fn(rng, sk);
    // Post-order on existing leaves only. Wrapping *during* a pre-order
    // walk would descend into the `+ 0` we just built and blow the stack.
    wrap_i32_leaves(&mut target.ret);
    wrap_i32_leaves_stmts(&mut target.body);
}

fn wrap_i32_leaves_stmts(stmts: &mut [Stmt]) {
    for st in stmts {
        match st {
            Stmt::LetMut { init, .. } => wrap_i32_leaves(init),
            Stmt::Assign { value, .. } => wrap_i32_leaves(value),
            Stmt::If {
                cond,
                then_s,
                else_s,
            } => {
                wrap_i32_leaves(cond);
                wrap_i32_leaves_stmts(then_s);
                if let Some(e) = else_s {
                    wrap_i32_leaves_stmts(e);
                }
            }
            Stmt::For { body, .. } => wrap_i32_leaves_stmts(body),
            Stmt::Print(e) => wrap_i32_leaves(e),
            Stmt::Break => {}
        }
    }
}

fn wrap_i32_leaves(e: &mut Expr) {
    match &mut e.kind {
        ExprKind::Bin { lhs, rhs, .. } => {
            wrap_i32_leaves(lhs);
            wrap_i32_leaves(rhs);
        }
        ExprKind::Un { inner, .. } => wrap_i32_leaves(inner),
        ExprKind::Call { arg, .. } => wrap_i32_leaves(arg),
        ExprKind::LitI32(_) | ExprKind::Var(_) if e.ty == STy::I32 => {
            *e = identity_wrap(e.clone());
        }
        _ => {}
    }
}

fn identity_wrap(e: Expr) -> Expr {
    // e + 0  — stresses algebraic / const-fold, keeps the value.
    bin("+", STy::I32, e, lit_i32(0))
}

fn insert_dead_let(rng: &mut FuzzRng, sk: &mut ProgramSketch) {
    let target = pick_fn(rng, sk);
    let name = format!("dead{}", rng.int(0, 99));
    target.body.push(Stmt::LetMut {
        name,
        init: lit_i32(rng.int(0, 5)),
    });
}

fn wrap_body_if_true(rng: &mut FuzzRng, sk: &mut ProgramSketch) {
    let target = pick_fn(rng, sk);
    // Wrap a single non-binding statement so names used by `return`
    // stay in the enclosing block.
    let idx = target.body.iter().position(|s| {
        !matches!(s, Stmt::LetMut { .. } | Stmt::For { .. } | Stmt::Break)
    });
    if let Some(i) = idx {
        let st = target.body.remove(i);
        target.body.insert(
            i,
            Stmt::If {
                cond: lit_bool(true),
                then_s: vec![st],
                else_s: None,
            },
        );
    }
    let _ = rng;
}

fn tweak_literals(rng: &mut FuzzRng, sk: &mut ProgramSketch) {
    let target = pick_fn(rng, sk);
    walk_exprs_fn(target, &mut |e| {
        if let ExprKind::LitI32(n) = &mut e.kind {
            *n = rng.int(0, 12);
        }
    });
}

fn swap_same_sort_binop(rng: &mut FuzzRng, sk: &mut ProgramSketch) {
    let arith = ["+", "-", "*"];
    let cmp = ["<", ">", "<=", ">=", "==", "!="];
    let logic = ["&&", "||"];
    let target = pick_fn(rng, sk);
    walk_exprs_fn(target, &mut |e| {
        if let ExprKind::Bin { op, .. } = &mut e.kind {
            if arith.contains(op) {
                *op = arith[rng.int(0, 2) as usize];
            } else if cmp.contains(op) {
                *op = cmp[rng.int(0, 5) as usize];
            } else if logic.contains(op) {
                *op = logic[rng.int(0, 1) as usize];
            }
        }
    });
}

fn tweak_for_bounds(rng: &mut FuzzRng, sk: &mut ProgramSketch) {
    let target = pick_fn(rng, sk);
    walk_stmts(&mut target.body, &mut |st| {
        if let Stmt::For { lo, hi, .. } = st {
            *lo = rng.int(0, 2);
            *hi = *lo + rng.int(0, 3);
        }
    });
}

fn retarget_call(rng: &mut FuzzRng, sk: &mut ProgramSketch) {
    let n = sk.helpers.len();
    if n == 0 {
        return;
    }
    let target = pick_fn(rng, sk);
    walk_exprs_fn(target, &mut |e| {
        if let ExprKind::Call { name, .. } = &mut e.kind {
            *name = format!("h{}", rng.int(0, (n - 1) as i32));
        }
    });
}

fn pick_fn<'a>(rng: &mut FuzzRng, sk: &'a mut ProgramSketch) -> &'a mut FnSketch {
    if !sk.helpers.is_empty() && rng.bool() {
        let i = rng.int(0, (sk.helpers.len() - 1) as i32) as usize;
        &mut sk.helpers[i]
    } else {
        &mut sk.main
    }
}

fn walk_stmts(stmts: &mut [Stmt], f: &mut impl FnMut(&mut Stmt)) {
    for st in stmts {
        f(st);
        match st {
            Stmt::If { then_s, else_s, .. } => {
                walk_stmts(then_s, f);
                if let Some(e) = else_s {
                    walk_stmts(e, f);
                }
            }
            Stmt::For { body, .. } => walk_stmts(body, f),
            _ => {}
        }
    }
}

fn walk_exprs_fn(func: &mut FnSketch, f: &mut impl FnMut(&mut Expr)) {
    walk_exprs_stmts(&mut func.body, f);
    walk_expr(&mut func.ret, f);
}

fn walk_exprs_stmts(stmts: &mut [Stmt], f: &mut impl FnMut(&mut Expr)) {
    for st in stmts {
        match st {
            Stmt::LetMut { init, .. } => walk_expr(init, f),
            Stmt::Assign { value, .. } => walk_expr(value, f),
            Stmt::If {
                cond,
                then_s,
                else_s,
            } => {
                walk_expr(cond, f);
                walk_exprs_stmts(then_s, f);
                if let Some(e) = else_s {
                    walk_exprs_stmts(e, f);
                }
            }
            Stmt::For { body, .. } => walk_exprs_stmts(body, f),
            Stmt::Print(e) => walk_expr(e, f),
            Stmt::Break => {}
        }
    }
}

fn walk_expr(e: &mut Expr, f: &mut impl FnMut(&mut Expr)) {
    f(e);
    match &mut e.kind {
        ExprKind::Bin { lhs, rhs, .. } => {
            walk_expr(lhs, f);
            walk_expr(rhs, f);
        }
        ExprKind::Un { inner, .. } => walk_expr(inner, f),
        ExprKind::Call { arg, .. } => walk_expr(arg, f),
        _ => {}
    }
}

/// Names defined by a statement that later statements may refer to.
fn defines(st: &Stmt) -> Option<&str> {
    match st {
        Stmt::LetMut { name, .. } => Some(name.as_str()),
        _ => None,
    }
}

fn uses_in_expr(e: &Expr, out: &mut HashSet<String>) {
    match &e.kind {
        ExprKind::Var(n) => {
            out.insert(n.clone());
        }
        ExprKind::Bin { lhs, rhs, .. } => {
            uses_in_expr(lhs, out);
            uses_in_expr(rhs, out);
        }
        ExprKind::Un { inner, .. } => uses_in_expr(inner, out),
        ExprKind::Call { name, arg, .. } => {
            out.insert(name.clone());
            uses_in_expr(arg, out);
        }
        _ => {}
    }
}

fn uses_in_stmts(stmts: &[Stmt], out: &mut HashSet<String>) {
    for st in stmts {
        match st {
            Stmt::LetMut { init, .. } => uses_in_expr(init, out),
            Stmt::Assign { name, value } => {
                out.insert(name.clone());
                uses_in_expr(value, out);
            }
            Stmt::If {
                cond,
                then_s,
                else_s,
            } => {
                uses_in_expr(cond, out);
                uses_in_stmts(then_s, out);
                if let Some(e) = else_s {
                    uses_in_stmts(e, out);
                }
            }
            Stmt::For { body, .. } => uses_in_stmts(body, out),
            Stmt::Print(e) => uses_in_expr(e, out),
            Stmt::Break => {}
        }
    }
}

/// Drop statements that nothing after them (or the return) refers to.
pub fn shrink_sketch(sk: &ProgramSketch) -> ProgramSketch {
    let mut out = sk.clone();
    shrink_fn(&mut out.main);
    let mut used = HashSet::new();
    uses_in_stmts(&out.main.body, &mut used);
    uses_in_expr(&out.main.ret, &mut used);
    for other in &out.helpers {
        uses_in_stmts(&other.body, &mut used);
        uses_in_expr(&other.ret, &mut used);
    }
    out.helpers.retain(|h| used.contains(&h.name));
    out
}

fn shrink_fn(func: &mut FnSketch) {
    let mut kept: Vec<Stmt> = Vec::new();
    for (i, st) in func.body.iter().enumerate() {
        if let Some(name) = defines(st) {
            let mut later = HashSet::new();
            uses_in_stmts(&func.body[i + 1..], &mut later);
            uses_in_expr(&func.ret, &mut later);
            if !later.contains(name) && name != "acc" {
                continue;
            }
        }
        kept.push(st.clone());
    }
    func.body = kept;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuzz::rng::FuzzRng;

    #[test]
    fn sketch_renders_main() {
        let mut rng = FuzzRng::new(1);
        let src = gen_sketch(&mut rng).render();
        assert!(src.contains("fn main"));
        assert!(src.contains("return"));
    }

    #[test]
    fn aspect_mutant_still_has_main() {
        let mut rng = FuzzRng::new(3);
        let sk = gen_sketch(&mut rng);
        let m = mutate_aspect(&mut rng, &sk, true);
        assert!(m.render().contains("fn main"));
        assert!(m.main.ret.ty == STy::I32);
    }
}

//! Exhaustiveness and reachability of `match` arms (pattern-matrix
//! specialisation, after Maranget's "Warnings for pattern matching").
//!
//! Patterns are first simplified to constructors and wildcards. A column's
//! type decides whether its constructors form a *complete signature*
//! (enum variants, the single tuple constructor, `true` / `false`); integer,
//! `char` and `string` literals never do, so those columns always need a
//! wildcard row.

use super::HirPattern;
use crate::ast::Literal;
use crate::ty::Type;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ctor {
    Variant(usize),
    Tuple,
    Bool(bool),
    Int(i64),
    Char(char),
    Str(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pat {
    Wild,
    Ctor(Ctor, Vec<Pat>),
}

/// A pattern that is not covered, printable as source.
#[derive(Debug, Clone)]
enum Wit {
    Wild,
    Node { head: String, args: Vec<Wit>, tuple: bool },
}

impl Wit {
    fn render(&self) -> String {
        match self {
            Wit::Wild => "_".into(),
            Wit::Node { head, args, tuple } => {
                if args.is_empty() && !*tuple {
                    head.clone()
                } else {
                    let a: Vec<String> = args.iter().map(Wit::render).collect();
                    format!("{head}({})", a.join(", "))
                }
            }
        }
    }
}

/// Work limit for one `match` (recursive calls plus rows visited). Hitting
/// it makes the caller report the match as too complex rather than guess.
const BUDGET: usize = 4_000_000;

pub struct Checker {
    steps: usize,
}

/// `Err(())` = work budget exhausted.
type Res<T> = Result<T, ()>;

pub fn simplify(p: &HirPattern) -> Pat {
    match p {
        HirPattern::Wildcard | HirPattern::Binding { .. } => Pat::Wild,
        HirPattern::Literal { lit, .. } => match lit {
            Literal::Int(v) => Pat::Ctor(Ctor::Int(*v), Vec::new()),
            Literal::Bool(v) => Pat::Ctor(Ctor::Bool(*v), Vec::new()),
            Literal::Char(c) => Pat::Ctor(Ctor::Char(*c), Vec::new()),
            Literal::String(s) => Pat::Ctor(Ctor::Str(s.clone()), Vec::new()),
            // `()` has exactly one value: it matches everything.
            Literal::Unit | Literal::Float(_) => Pat::Wild,
        },
        HirPattern::Variant { tag, fields, .. } => {
            Pat::Ctor(Ctor::Variant(*tag), fields.iter().map(simplify).collect())
        }
        HirPattern::Tuple { elems, .. } => {
            Pat::Ctor(Ctor::Tuple, elems.iter().map(simplify).collect())
        }
    }
}

/// True when the pattern mentions an enum variant anywhere.
pub fn has_variant(p: &Pat) -> bool {
    match p {
        Pat::Wild => false,
        Pat::Ctor(Ctor::Variant(_), _) => true,
        Pat::Ctor(_, args) => args.iter().any(has_variant),
    }
}

/// Every constructor of `ty` with its argument types, when the set is finite
/// and small enough to enumerate.
fn signature(ty: &Type) -> Option<Vec<(Ctor, Vec<Type>)>> {
    match ty {
        Type::Enum { variants, .. } => Some(
            variants
                .iter()
                .enumerate()
                .map(|(i, (_, p))| (Ctor::Variant(i), p.clone()))
                .collect(),
        ),
        Type::Tuple(ts) => Some(vec![(Ctor::Tuple, ts.clone())]),
        Type::Bool => Some(vec![
            (Ctor::Bool(false), Vec::new()),
            (Ctor::Bool(true), Vec::new()),
        ]),
        _ => None,
    }
}

fn ctor_args(c: &Ctor, ty: &Type) -> Vec<Type> {
    match (c, ty) {
        (Ctor::Variant(i), Type::Enum { variants, .. }) => {
            variants.get(*i).map(|(_, p)| p.clone()).unwrap_or_default()
        }
        (Ctor::Tuple, Type::Tuple(ts)) => ts.clone(),
        _ => Vec::new(),
    }
}

fn head_text(c: &Ctor, ty: &Type) -> String {
    match (c, ty) {
        (Ctor::Variant(i), Type::Enum { name, variants }) => match variants.get(*i) {
            Some((v, _)) => format!("{name}::{v}"),
            None => "_".into(),
        },
        (Ctor::Tuple, _) => String::new(),
        (Ctor::Bool(b), _) => b.to_string(),
        (Ctor::Int(v), _) => v.to_string(),
        (Ctor::Char(ch), _) => format!("{ch:?}"),
        (Ctor::Str(s), _) => format!("{s:?}"),
        _ => "_".into(),
    }
}

fn specialise(rows: &[Vec<Pat>], c: &Ctor, arity: usize) -> Vec<Vec<Pat>> {
    let mut out = Vec::new();
    for row in rows {
        match &row[0] {
            Pat::Ctor(c2, args) if c2 == c => {
                let mut r = args.clone();
                r.extend_from_slice(&row[1..]);
                out.push(r);
            }
            Pat::Ctor(..) => {}
            Pat::Wild => {
                let mut r = vec![Pat::Wild; arity];
                r.extend_from_slice(&row[1..]);
                out.push(r);
            }
        }
    }
    out
}

fn default_rows(rows: &[Vec<Pat>]) -> Vec<Vec<Pat>> {
    rows.iter()
        .filter(|r| matches!(r[0], Pat::Wild))
        .map(|r| r[1..].to_vec())
        .collect()
}

fn heads(rows: &[Vec<Pat>]) -> Vec<Ctor> {
    let mut seen: HashSet<&Ctor> = HashSet::new();
    let mut hs: Vec<Ctor> = Vec::new();
    for r in rows {
        if let Pat::Ctor(c, _) = &r[0] {
            if seen.insert(c) {
                hs.push(c.clone());
            }
        }
    }
    hs
}

impl Checker {
    pub fn new() -> Self {
        Checker { steps: 0 }
    }

    fn tick(&mut self, n: usize) -> Res<()> {
        self.steps += n + 1;
        if self.steps > BUDGET {
            Err(())
        } else {
            Ok(())
        }
    }

    /// An example of a value of `ty` that no arm matches, rendered as a
    /// pattern; `None` when the arms are exhaustive.
    pub fn missing(&mut self, arms: &[Pat], ty: &Type) -> Res<Option<String>> {
        let rows: Vec<Vec<Pat>> = arms.iter().map(|p| vec![p.clone()]).collect();
        let w = self.missing_rows(rows, &[ty.clone()])?;
        Ok(w.map(|w| w[0].render()))
    }

    fn missing_rows(&mut self, rows: Vec<Vec<Pat>>, tys: &[Type]) -> Res<Option<Vec<Wit>>> {
        self.tick(rows.len())?;
        if tys.is_empty() {
            return Ok(if rows.is_empty() { Some(Vec::new()) } else { None });
        }
        let ty = &tys[0];
        let rest = &tys[1..];
        let hs = heads(&rows);
        let sig = signature(ty);
        let complete = match &sig {
            Some(sig) => !hs.is_empty() && sig.iter().all(|(c, _)| hs.contains(c)),
            None => false,
        };
        if complete {
            for (c, arg_tys) in sig.unwrap_or_default() {
                let spec = specialise(&rows, &c, arg_tys.len());
                let mut new_tys = arg_tys.clone();
                new_tys.extend_from_slice(rest);
                if let Some(mut w) = self.missing_rows(spec, &new_tys)? {
                    let tail = w.split_off(arg_tys.len());
                    let node = Wit::Node {
                        head: head_text(&c, ty),
                        args: w,
                        tuple: c == Ctor::Tuple,
                    };
                    let mut out = vec![node];
                    out.extend(tail);
                    return Ok(Some(out));
                }
            }
            return Ok(None);
        }
        let Some(mut w) = self.missing_rows(default_rows(&rows), rest)? else {
            return Ok(None);
        };
        let head = match &sig {
            Some(sig) if !hs.is_empty() => sig
                .iter()
                .find(|(c, _)| !hs.contains(c))
                .map(|(c, a)| Wit::Node {
                    head: head_text(c, ty),
                    args: vec![Wit::Wild; a.len()],
                    tuple: *c == Ctor::Tuple,
                })
                .unwrap_or(Wit::Wild),
            _ => Wit::Wild,
        };
        w.insert(0, head);
        Ok(Some(w))
    }

    /// Is `q` matched by some value that none of `rows` matches?
    pub fn useful(&mut self, rows: &[Pat], q: &Pat, ty: &Type) -> Res<bool> {
        let rows: Vec<Vec<Pat>> = rows.iter().map(|p| vec![p.clone()]).collect();
        self.useful_rows(&rows, &[q.clone()], &[ty.clone()])
    }

    fn useful_rows(&mut self, rows: &[Vec<Pat>], q: &[Pat], tys: &[Type]) -> Res<bool> {
        self.tick(rows.len())?;
        if q.is_empty() {
            return Ok(rows.is_empty());
        }
        let ty = &tys[0];
        let rest = &tys[1..];
        match &q[0] {
            Pat::Ctor(c, args) => {
                let spec = specialise(rows, c, args.len());
                let mut q2 = args.clone();
                q2.extend_from_slice(&q[1..]);
                let mut tys2 = ctor_args(c, ty);
                tys2.resize(args.len(), Type::Error);
                tys2.extend_from_slice(rest);
                self.useful_rows(&spec, &q2, &tys2)
            }
            Pat::Wild => {
                let hs = heads(rows);
                let sig = signature(ty);
                let complete = match &sig {
                    Some(sig) => !hs.is_empty() && sig.iter().all(|(c, _)| hs.contains(c)),
                    None => false,
                };
                if complete {
                    for (c, arg_tys) in sig.unwrap_or_default() {
                        let spec = specialise(rows, &c, arg_tys.len());
                        let mut q2 = vec![Pat::Wild; arg_tys.len()];
                        q2.extend_from_slice(&q[1..]);
                        let mut tys2 = arg_tys.clone();
                        tys2.extend_from_slice(rest);
                        if self.useful_rows(&spec, &q2, &tys2)? {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                } else {
                    self.useful_rows(&default_rows(rows), &q[1..], rest)
                }
            }
        }
    }
}

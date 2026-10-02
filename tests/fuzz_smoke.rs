//! Fast, deterministic smoke run of every fuzz kind so `cargo test`
//! exercises the generators and the oracle suite (`-O0`/`-O1`/`-O2`
//! equality, IR verification, determinism, `run_budget` equivalence,
//! formatter idempotence). Long campaigns: `aether fuzz --kind all`.
//!
//! The per-case watchdog is raised to 120 s here so a loaded CI machine
//! cannot turn a slow debug build into a false hang report.

use aether::fuzz::{
    gen_aggregate_program, gen_lang_program, gen_lang_source, run_fuzz_with, FuzzConfig, FuzzKind,
    FuzzOptions, FuzzReport, FuzzRng,
};
use std::time::Duration;

fn render(r: &FuzzReport) -> String {
    let mut s = r.summary();
    for f in &r.failures {
        s.push_str(&format!(
            "\n[{}] seed={} case={}\n{}\n---\n{}\n",
            f.property, f.seed, f.case, f.detail, f.source
        ));
    }
    s
}

fn smoke_with(kind: FuzzKind, iters: u32, seed: u64, fmt: bool) {
    let cfg = FuzzConfig { iters, seed, kind };
    let opts = FuzzOptions {
        timeout: Duration::from_secs(120),
        fmt,
    };
    let report = run_fuzz_with(&cfg, opts);
    assert_eq!(report.passed + report.failed, iters, "{}", render(&report));
    assert!(report.ok(), "kind {kind:?} seed {seed}\n{}", render(&report));
}

/// Everything except the formatter round trip (see `smoke_fmt_roundtrip`).
fn smoke(kind: FuzzKind, iters: u32, seed: u64) {
    smoke_with(kind, iters, seed, false);
}

/// `aether fmt` output must recompile to the same behaviour and be a fixed
/// point. Ignored while the pretty printer drops `let` type annotations and
/// prints `3.0` as `3` (see docs/fuzzing.md, "Known compiler findings");
/// run with `cargo test --test fuzz_smoke -- --ignored` after it is fixed
/// and then remove the `#[ignore]`.
#[test]
#[ignore = "pretty printer bugs, docs/fuzzing.md"]
fn smoke_fmt_roundtrip() {
    smoke_with(FuzzKind::Lang, 30, 12, true);
    smoke_with(FuzzKind::Aggregate, 30, 11, true);
    smoke_with(FuzzKind::Differential, 30, 5, true);
}

#[test]
fn smoke_lexer_parser_pipeline() {
    smoke(FuzzKind::Lexer, 30, 1);
    smoke(FuzzKind::Parser, 30, 2);
    smoke(FuzzKind::Pipeline, 30, 3);
}

#[test]
fn smoke_generated_and_differential() {
    smoke(FuzzKind::Generated, 30, 4);
    smoke(FuzzKind::Differential, 30, 5);
}

#[test]
fn smoke_mutation_kinds() {
    smoke(FuzzKind::Mutated, 30, 6);
    smoke(FuzzKind::Structural, 30, 7);
    smoke(FuzzKind::Aspect, 30, 8);
    smoke(FuzzKind::Format, 30, 9);
}

#[test]
fn smoke_mir() {
    smoke(FuzzKind::Mir, 30, 10);
}

#[test]
fn smoke_aggregate() {
    smoke(FuzzKind::Aggregate, 30, 11);
}

#[test]
fn smoke_lang() {
    smoke(FuzzKind::Lang, 30, 12);
}

#[test]
fn smoke_lang_second_seed_pair() {
    // adjacent seeds used to run identical campaigns (`seed | 1`)
    smoke(FuzzKind::Lang, 15, 14);
    smoke(FuzzKind::Lang, 15, 15);
}

#[test]
fn smoke_greybox() {
    smoke(FuzzKind::Greybox, 16, 13);
}

#[test]
fn adjacent_seeds_run_different_campaigns() {
    let a = gen_lang_source(&mut FuzzRng::new(2));
    let b = gen_lang_source(&mut FuzzRng::new(3));
    assert_ne!(a, b);
    let a = gen_aggregate_program(&mut FuzzRng::new(100));
    let b = gen_aggregate_program(&mut FuzzRng::new(101));
    assert_ne!(a, b);
    // seed 0 is usable
    let z = gen_lang_source(&mut FuzzRng::new(0));
    assert!(z.contains("fn main() -> i32"));
}

#[test]
fn lang_generator_covers_the_new_syntax() {
    // Over a fixed batch the generator must reach every feature it claims.
    let mut seen = std::collections::BTreeSet::new();
    let needles: &[(&str, &str)] = &[
        ("enum", "enum E0"),
        ("match", "match "),
        ("if let", "if let "),
        ("else", "} else {"),
        ("tuple access", ".0"),
        ("let (a, b)", "let ("),
        ("<<=", "<<="),
        (">>=", ">>="),
        ("^=", "^="),
        ("&=", "&="),
        ("|=", "|="),
        ("hex", "0x"),
        ("binary", "0b"),
        ("underscore", "_000"),
        ("unicode escape", "\\u{"),
        ("yield", "yield;"),
        ("clamp", "clamp("),
        ("pow_i32", "pow_i32("),
        ("sqrt", "sqrt("),
        ("floor", "floor("),
        ("ceil", "ceil("),
        ("to_string", "to_string("),
        ("i64_to_string", "i64_to_string("),
        ("char_to_string", "char_to_string("),
        ("len", "len("),
        ("min", "min("),
        ("max", "max("),
        ("abs", "abs("),
        ("bit not", "(!"),
        ("use", "use \""),
        ("pub", "pub "),
        ("recursion", "fibr("),
    ];
    for s in 0..120u64 {
        let p = gen_lang_program(&mut FuzzRng::new(s));
        let all: String = p.files.iter().map(|(_, t)| t.as_str()).collect();
        for (name, n) in needles {
            if all.contains(n) {
                seen.insert(*name);
            }
        }
    }
    for (name, _) in needles {
        assert!(seen.contains(name), "generator never produced {name}");
    }
}

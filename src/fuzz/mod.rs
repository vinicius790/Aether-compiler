//! Property-based fuzzing for the Aether pipeline.
//!
//! No external fuzzer crate: a deterministic xorshift64* generator plus a
//! grammar-driven program synthesizer. Failures print the seed so they can
//! be replayed with `aether fuzz --seed N`.
//!
//! Properties
//! ----------
//! 1. **No panic** — lexer, parser and the full driver never abort on
//!    arbitrary bytes or mutated source.
//! 2. **Lexer well-formedness** — the token stream ends in `Eof`, spans are
//!    monotonic and inside the file (or at EOF).
//! 3. **Well-typed synthesis** — programs drawn from the grammar compile.
//! 4. **Differential optimisation** — `-O0` and `-O2` agree on the value
//!    returned by `main` and on captured stdout.
//! 5. **Mutation of valid programs** — bit-flips and junk insertion still
//!    do not panic.

mod format;
mod gen;
mod greybox;
mod mir;
mod mutate;
mod rng;
mod sketch;

pub use gen::gen_program;
pub use greybox::{run_greybox, GreyboxReport};
pub use mir::{check_ir, decoy_stats, gen_ir};
pub use mutate::{crossover, mutate_structural, shrink};
pub use rng::FuzzRng;
pub use sketch::{crossover_sketch, gen_sketch, mutate_aspect, ProgramSketch};

use crate::driver::{compile_source, run_compiled, CompileOptions};
use crate::lexer::tokenize;
use crate::parser::parse;
use crate::span::FileId;
use crate::token::TokenKind;
use crate::vm::Value;
use std::panic::{catch_unwind, AssertUnwindSafe};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FuzzKind {
    All,
    Lexer,
    Parser,
    Pipeline,
    Generated,
    Differential,
    Mutated,
    Structural,
    Aspect,
    Mir,
    Greybox,
    Format,
}

impl FuzzKind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "all" => FuzzKind::All,
            "lexer" => FuzzKind::Lexer,
            "parser" => FuzzKind::Parser,
            "pipeline" => FuzzKind::Pipeline,
            "gen" | "generated" => FuzzKind::Generated,
            "diff" | "differential" => FuzzKind::Differential,
            "mut" | "mutated" => FuzzKind::Mutated,
            "struct" | "structural" => FuzzKind::Structural,
            "aspect" | "ap" => FuzzKind::Aspect,
            "mir" | "ir" => FuzzKind::Mir,
            "grey" | "greybox" | "graybox" => FuzzKind::Greybox,
            "format" | "fmt" | "grammar" => FuzzKind::Format,
            _ => return None,
        })
    }

    fn suite(self) -> Vec<FuzzKind> {
        if self == FuzzKind::All {
            vec![
                FuzzKind::Lexer,
                FuzzKind::Parser,
                FuzzKind::Pipeline,
                FuzzKind::Generated,
                FuzzKind::Differential,
                FuzzKind::Mutated,
                FuzzKind::Structural,
                FuzzKind::Aspect,
                FuzzKind::Mir,
            ]
        } else {
            vec![self]
        }
    }
}

#[derive(Debug, Clone)]
pub struct FuzzConfig {
    pub iters: u32,
    pub seed: u64,
    pub kind: FuzzKind,
}

impl Default for FuzzConfig {
    fn default() -> Self {
        FuzzConfig {
            iters: 80,
            seed: 0xA37_E4_00,
            kind: FuzzKind::All,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FuzzFailure {
    pub property: &'static str,
    pub seed: u64,
    pub case: u32,
    pub detail: String,
    pub source: String,
}

#[derive(Debug, Clone, Default)]
pub struct FuzzReport {
    pub passed: u32,
    pub failed: u32,
    pub failures: Vec<FuzzFailure>,
    pub extra: String,
}

impl FuzzReport {
    pub fn ok(&self) -> bool {
        self.failed == 0
    }

    pub fn summary(&self) -> String {
        format!(
            "fuzz: {} passed, {} failed\n{}",
            self.passed, self.failed, self.extra
        )
    }
}

pub fn run_fuzz(cfg: &FuzzConfig) -> FuzzReport {
    if cfg.kind == FuzzKind::Greybox {
        let g = run_greybox(cfg.iters, cfg.seed);
        let mut report = FuzzReport::default();
        report.passed = cfg.iters.saturating_sub(g.failures);
        report.failed = g.failures;
        report.extra = g.summary();
        if let Some(detail) = g.last_failure.clone() {
            report.failures.push(FuzzFailure {
                property: "greybox",
                seed: cfg.seed,
                case: 0,
                detail: format!("{}{detail}", report.extra),
                source: String::new(),
            });
        }
        return report;
    }
    let mut report = FuzzReport::default();
    for kind in cfg.kind.suite() {
        let mut rng = FuzzRng::new(cfg.seed ^ kind_salt(kind));
        for case in 0..cfg.iters {
            let case_seed = rng.next_u64();
            let mut case_rng = FuzzRng::new(case_seed);
            let result = match kind {
                FuzzKind::Lexer => prop_lexer(&mut case_rng),
                FuzzKind::Parser => prop_parser(&mut case_rng),
                FuzzKind::Pipeline => prop_pipeline(&mut case_rng),
                FuzzKind::Generated => prop_generated(&mut case_rng),
                FuzzKind::Differential => prop_differential(&mut case_rng),
                FuzzKind::Mutated => prop_mutated(&mut case_rng),
                FuzzKind::Structural => prop_structural(&mut case_rng),
                FuzzKind::Aspect => prop_aspect(&mut case_rng),
                FuzzKind::Mir => prop_mir(&mut case_rng),
                FuzzKind::Format => prop_format(&mut case_rng),
                FuzzKind::Greybox | FuzzKind::All => unreachable!(),
            };
            match result {
                Ok(()) => report.passed += 1,
                Err(mut fail) => {
                    fail.seed = case_seed;
                    fail.case = case;
                    report.failed += 1;
                    if report.failures.len() < 8 {
                        report.failures.push(fail);
                    }
                }
            }
        }
    }
    report
}

fn kind_salt(kind: FuzzKind) -> u64 {
    match kind {
        FuzzKind::All => 0,
        FuzzKind::Lexer => 0x11,
        FuzzKind::Parser => 0x22,
        FuzzKind::Pipeline => 0x33,
        FuzzKind::Generated => 0x44,
        FuzzKind::Differential => 0x55,
        FuzzKind::Mutated => 0x66,
        FuzzKind::Structural => 0x77,
        FuzzKind::Aspect => 0x88,
        FuzzKind::Mir => 0x99,
        FuzzKind::Greybox => 0xAA,
        FuzzKind::Format => 0xBB,
    }
}

fn prop_format(rng: &mut FuzzRng) -> Result<(), FuzzFailure> {
    let src = format::gen_format(rng);
    let opts = CompileOptions {
        opt_level: 0,
        color: false,
    };
    let caught = catch_unwind(AssertUnwindSafe(|| {
        let _ = tokenize(FileId(0), &src);
        let _ = compile_source("<fmt>", &src, &opts);
    }));
    match caught {
        Ok(()) => Ok(()),
        Err(_) => Err(fail("format_no_panic", "grammar-tape input panicked the compiler", &src)),
    }
}

fn prop_lexer(rng: &mut FuzzRng) -> Result<(), FuzzFailure> {
    let src = random_source(rng);
    let caught = catch_unwind(AssertUnwindSafe(|| tokenize(FileId(0), &src)));
    let (tokens, _diags) = match caught {
        Ok(v) => v,
        Err(_) => {
            return Err(fail(
                "lexer_no_panic",
                format!("lexer panicked on {} bytes", src.len()),
                &src,
            ))
        }
    };
    if tokens.is_empty() {
        return Err(fail("lexer_eof", "empty token stream", &src));
    }
    if tokens.last().map(|t| t.kind) != Some(TokenKind::Eof) {
        return Err(fail("lexer_eof", "token stream does not end with Eof", &src));
    }
    let mut prev_end = 0u32;
    let len = src.len() as u32;
    for (i, tok) in tokens.iter().enumerate() {
        if tok.span.start.0 > tok.span.end.0 {
            return Err(fail(
                "lexer_span",
                format!("token {i} has inverted span"),
                &src,
            ));
        }
        if tok.kind != TokenKind::Eof && tok.span.end.0 > len {
            return Err(fail(
                "lexer_span",
                format!("token {i} ends past source"),
                &src,
            ));
        }
        if tok.span.start.0 < prev_end && tok.kind != TokenKind::Eof {
            return Err(fail(
                "lexer_span",
                format!("token {i} overlaps previous token"),
                &src,
            ));
        }
        if tok.kind != TokenKind::Eof {
            prev_end = tok.span.end.0;
        }
        if tok.kind != TokenKind::Eof && tok.span.line == 0 {
            return Err(fail("lexer_span", format!("token {i} has line 0"), &src));
        }
    }
    Ok(())
}

fn prop_parser(rng: &mut FuzzRng) -> Result<(), FuzzFailure> {
    let src = random_source(rng);
    let caught = catch_unwind(AssertUnwindSafe(|| {
        let (tokens, _) = tokenize(FileId(0), &src);
        parse(tokens)
    }));
    match caught {
        Ok(_) => Ok(()),
        Err(_) => Err(fail(
            "parser_no_panic",
            format!("parser panicked on {} bytes", src.len()),
            &src,
        )),
    }
}

fn prop_pipeline(rng: &mut FuzzRng) -> Result<(), FuzzFailure> {
    let src = random_source(rng);
    let opts = CompileOptions {
        opt_level: 1,
        color: false,
    };
    let caught = catch_unwind(AssertUnwindSafe(|| compile_source("<fuzz>", &src, &opts)));
    match caught {
        Ok(_) => Ok(()),
        Err(_) => Err(fail(
            "pipeline_no_panic",
            "compile_source panicked",
            &src,
        )),
    }
}

fn prop_generated(rng: &mut FuzzRng) -> Result<(), FuzzFailure> {
    let src = gen_program(rng);
    let opts = CompileOptions {
        opt_level: 0,
        color: false,
    };
    let caught = catch_unwind(AssertUnwindSafe(|| compile_source("<gen>", &src, &opts)));
    let compiled = match caught {
        Ok(c) => c,
        Err(_) => return Err(fail("gen_compile", "generated program panicked the compiler", &src)),
    };
    if compiled.diags.has_errors() {
        return Err(fail(
            "gen_well_typed",
            format!(
                "generated program rejected:\n{}",
                compiled.diags.render(&compiled.session, false)
            ),
            &src,
        ));
    }
    let (tokens, _) = tokenize(FileId(0), &src);
    if tokens.iter().any(|t| t.kind == TokenKind::Invalid) {
        return Err(fail(
            "gen_tokens",
            "generated program produced Invalid tokens",
            &src,
        ));
    }
    Ok(())
}

fn prop_differential(rng: &mut FuzzRng) -> Result<(), FuzzFailure> {
    let src = gen_program(rng);
    match eval_levels(&src) {
        Ok((v0, o0, v2, o2)) => {
            if v0 != v2 {
                return Err(fail(
                    "opt_equiv_value",
                    format!("-O0 returned {v0:?}, -O2 returned {v2:?}"),
                    &src,
                ));
            }
            if o0 != o2 {
                return Err(fail(
                    "opt_equiv_stdout",
                    format!("stdout differs:\n-O0: {o0:?}\n-O2: {o2:?}"),
                    &src,
                ));
            }
            Ok(())
        }
        Err(e) => Err(fail("opt_equiv_run", e, &src)),
    }
}

fn prop_structural(rng: &mut FuzzRng) -> Result<(), FuzzFailure> {
    let base = gen_program(rng);
    let donor = gen_program(rng);
    let src = if rng.bool() {
        let n = rng.int(1, 4) as usize;
        mutate_structural(rng, &base, n)
    } else {
        let crossed = crossover(rng, &base, &donor);
        let n = rng.int(0, 2) as usize;
        mutate_structural(rng, &crossed, n)
    };
    let opts = CompileOptions {
        opt_level: 0,
        color: false,
    };
    let caught = catch_unwind(AssertUnwindSafe(|| compile_source("<struct>", &src, &opts)));
    let compiled = match caught {
        Ok(c) => c,
        Err(_) => {
            return Err(fail(
                "struct_no_panic",
                "structural mutant panicked the compiler",
                &src,
            ))
        }
    };
    // Structural mutants are *meant* to stay close to the grammar. A type
    // error is acceptable (e.g. deleting a `let` that a later use needs);
    // a panic is not. We still record well-typed rate implicitly: if it
    // compiled, run O0 vs O2.
    if !compiled.diags.has_errors() {
        if let Err(e) = eval_levels(&src).and_then(|(v0, o0, v2, o2)| {
            if v0 != v2 {
                Err(format!("-O0={v0:?} -O2={v2:?}"))
            } else if o0 != o2 {
                Err(format!("stdout {o0:?} vs {o2:?}"))
            } else {
                Ok(())
            }
        }) {
            let shrunk = shrink(&src, |cand| eval_levels(cand).map(|(a, _, b, _)| a != b).unwrap_or(false));
            return Err(fail(
                "struct_opt_equiv",
                format!("{e}\nshrunk to {} bytes", shrunk.len()),
                &shrunk,
            ));
        }
    }
    Ok(())
}

fn prop_aspect(rng: &mut FuzzRng) -> Result<(), FuzzFailure> {
    let base = gen_sketch(rng);
    let donor = gen_sketch(rng);
    let crossed = if rng.bool() {
        crossover_sketch(rng, &base, &donor)
    } else {
        base.clone()
    };
    let preserve = rng.bool();
    let mutant = mutate_aspect(rng, &crossed, preserve);
    let mutant = sketch::shrink_sketch(&mutant);
    let src = mutant.render();
    let opts = CompileOptions {
        opt_level: 0,
        color: false,
    };
    let caught = catch_unwind(AssertUnwindSafe(|| compile_source("<aspect>", &src, &opts)));
    let compiled = match caught {
        Ok(c) => c,
        Err(_) => {
            return Err(fail(
                "aspect_no_panic",
                "aspect-preserving mutant panicked the compiler",
                &src,
            ))
        }
    };
    if compiled.diags.has_errors() {
        return Err(fail(
            "aspect_well_typed",
            format!(
                "aspect mutant rejected:\n{}",
                compiled.diags.render(&compiled.session, false)
            ),
            &src,
        ));
    }
    if preserve {
        if let Err(e) = eval_levels(&src).and_then(|(v0, o0, v2, o2)| {
            if v0 != v2 {
                Err(format!("-O0={v0:?} -O2={v2:?}"))
            } else if o0 != o2 {
                Err(format!("stdout {o0:?} vs {o2:?}"))
            } else {
                Ok(())
            }
        }) {
            return Err(fail("aspect_opt_equiv", e, &src));
        }
    }
    Ok(())
}

fn prop_mir(rng: &mut FuzzRng) -> Result<(), FuzzFailure> {
    use crate::ir::dump_ir;
    let raw = gen_ir(rng);
    let module = if rng.bool() {
        raw
    } else {
        mir::mutate_ir(rng, &raw)
    };
    if let Err(e) = check_ir(&module) {
        return Err(fail("mir_well_formed", e, &dump_ir(&module)));
    }
    let caught = catch_unwind(AssertUnwindSafe(|| {
        let o0 = mir::eval_ir(module.clone(), 0);
        let o2 = mir::eval_ir(module.clone(), 2);
        (o0, o2)
    }));
    let (o0, o2) = match caught {
        Ok(v) => v,
        Err(_) => {
            return Err(fail(
                "mir_no_panic",
                "optimizer/VM panicked on generated IR",
                &dump_ir(&module),
            ))
        }
    };
    match (o0, o2) {
        (Ok((v0, s0)), Ok((v2, s2))) => {
            if v0 != v2 {
                return Err(fail(
                    "mir_opt_equiv_value",
                    format!("-O0={v0:?} -O2={v2:?}"),
                    &dump_ir(&module),
                ));
            }
            if s0 != s2 {
                return Err(fail(
                    "mir_opt_equiv_stdout",
                    format!("stdout {s0:?} vs {s2:?}"),
                    &dump_ir(&module),
                ));
            }
            Ok(())
        }
        (Err(a), Err(b)) if a == b => Ok(()),
        (a, b) => Err(fail(
            "mir_eval",
            format!("O0={a:?} O2={b:?}"),
            &dump_ir(&module),
        )),
    }
}

fn prop_mutated(rng: &mut FuzzRng) -> Result<(), FuzzFailure> {
    let base = gen_program(rng);
    let src = mutate_source(rng, &base);
    let opts = CompileOptions {
        opt_level: 0,
        color: false,
    };
    let caught = catch_unwind(AssertUnwindSafe(|| {
        let _ = tokenize(FileId(0), &src);
        let _ = compile_source("<mut>", &src, &opts);
    }));
    match caught {
        Ok(()) => Ok(()),
        Err(_) => Err(fail("mut_no_panic", "mutated source panicked the compiler", &src)),
    }
}

fn eval_levels(src: &str) -> Result<(Value, String, Value, String), String> {
    let run = |level: u8| {
        let opts = CompileOptions {
            opt_level: level,
            color: false,
        };
        let caught = catch_unwind(AssertUnwindSafe(|| compile_source("<diff>", src, &opts)));
        let mut compiled = caught.map_err(|_| format!("panic at -O{level}"))?;
        if compiled.diags.has_errors() {
            return Err(format!(
                "rejected at -O{level}:\n{}",
                compiled.diags.render(&compiled.session, false)
            ));
        }
        match run_compiled(&mut compiled) {
            Ok((v, out, _)) => Ok((v, out)),
            Err(e) => Err(format!("runtime -O{level}: {e}")),
        }
    };
    match (run(0), run(2)) {
        (Ok((v0, o0)), Ok((v2, o2))) => Ok((v0, o0, v2, o2)),
        (Err(a), Err(b)) => {
            let class = |s: &str| {
                if s.contains("step limit") {
                    "step"
                } else if s.contains("division") {
                    "div0"
                } else if s.contains("stack") {
                    "stack"
                } else if s.contains("rejected") {
                    "compile"
                } else {
                    "other"
                }
            };
            if class(&a) == class(&b) {
                Ok((Value::Unit, String::new(), Value::Unit, String::new()))
            } else {
                Err(format!("asymmetric failure:\n{a}\n{b}"))
            }
        }
        (Ok(_), Err(e)) => Err(format!("-O0 succeeded, -O2 failed: {e}")),
        (Err(e), Ok(_)) => Err(format!("-O2 succeeded, -O0 failed: {e}")),
    }
}

fn random_source(rng: &mut FuzzRng) -> String {
    match rng.int(0, 5) {
        0 => {
            let n = rng.int(0, 96) as usize;
            rng.random_bytes(n)
        }
        1 => {
            let n = rng.int(0, 96) as usize;
            rng.random_ascii(n)
        }
        2 => {
            let base = gen_program(rng);
            mutate_source(rng, &base)
        }
        3 => keyword_soup(rng),
        4 => corpus_case(rng.int(0, (CORPUS.len() as i32) - 1) as usize).to_string(),
        _ => {
            let mut s = gen_program(rng);
            let n = rng.int(0, 16) as usize;
            s.push_str(&rng.random_ascii(n));
            s
        }
    }
}

fn mutate_source(rng: &mut FuzzRng, src: &str) -> String {
    let mut bytes: Vec<u8> = src.as_bytes().to_vec();
    if bytes.is_empty() {
        return rng.random_ascii(8);
    }
    let n = rng.int(1, 6) as usize;
    for _ in 0..n {
        match rng.int(0, 4) {
            0 => {
                let i = rng.int(0, (bytes.len() - 1) as i32) as usize;
                bytes[i] ^= 1 << rng.int(0, 7);
            }
            1 => {
                let i = rng.int(0, bytes.len() as i32) as usize;
                bytes.insert(i, rng.int(1, 127) as u8);
            }
            2 if bytes.len() > 1 => {
                let i = rng.int(0, (bytes.len() - 1) as i32) as usize;
                bytes.remove(i);
            }
            3 => bytes.extend_from_slice(b"\n@#$\x00/*"),
            _ => {
                let i = rng.int(0, bytes.len() as i32) as usize;
                bytes.insert(i, b'}');
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn keyword_soup(rng: &mut FuzzRng) -> String {
    const KW: &[&str] = &[
        "fn", "let", "mut", "if", "else", "while", "for", "in", "return", "struct", "true",
        "false", "as", "(", ")", "{", "}", "[", "]", ";", ":", "->", "..", "+", "-", "*", "/",
        "==", "!=", "<", ">", "&&", "||", "main", "i32", "0", "1", "\"x\"",
    ];
    let n = rng.int(3, 24) as usize;
    let mut s = String::new();
    for i in 0..n {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(KW[rng.int(0, (KW.len() - 1) as i32) as usize]);
    }
    s
}

const CORPUS: &[&str] = &[
    "",
    "\0",
    "/*",
    "\"",
    "'",
    "fn",
    "fn main",
    "fn main() -> i32 {",
    "fn main() -> i32 { return",
    "fn main() -> i32 { return 1 + ; }",
    "fn main() -> i32 { let x = ; }",
    "fn main() -> i32 { 1 = 2; return 0; }",
    "fn main() -> i32 { break; return 0; }",
    "fn main() -> i32 { return true; }",
    "fn main() -> bool { return 1; }",
    "struct S { x: i32 }",
    "fn main() -> i32 { let a = [1, true]; return 0; }",
    "fn main() -> i32 { return 1 / 0; }",
    "fn f() -> i32 { }\nfn main() -> i32 { return 0; }",
    "fn main() -> i32 { while true { } return 0; }",
    "fn main() -> i32 { let mut x = 0; x = x; return x; }",
    "🚀 fn main() -> i32 { return 0; }",
    "fn main() -> i32 { return 0; }\nfn main() -> i32 { return 1; }",
];

fn corpus_case(i: usize) -> &'static str {
    CORPUS[i % CORPUS.len()]
}

fn fail(property: &'static str, detail: impl Into<String>, source: &str) -> FuzzFailure {
    let mut src = source.to_string();
    if src.len() > 800 {
        src.truncate(800);
        src.push_str("…");
    }
    FuzzFailure {
        property,
        seed: 0,
        case: 0,
        detail: detail.into(),
        source: src,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexer_properties_hold() {
        let report = run_fuzz(&FuzzConfig {
            iters: 40,
            seed: 7,
            kind: FuzzKind::Lexer,
        });
        assert!(report.ok(), "{}", format_failures(&report));
    }

    #[test]
    fn parser_properties_hold() {
        let report = run_fuzz(&FuzzConfig {
            iters: 40,
            seed: 11,
            kind: FuzzKind::Parser,
        });
        assert!(report.ok(), "{}", format_failures(&report));
    }

    #[test]
    fn pipeline_does_not_panic() {
        let report = run_fuzz(&FuzzConfig {
            iters: 30,
            seed: 13,
            kind: FuzzKind::Pipeline,
        });
        assert!(report.ok(), "{}", format_failures(&report));
    }

    #[test]
    fn generated_programs_typecheck() {
        let report = run_fuzz(&FuzzConfig {
            iters: 25,
            seed: 17,
            kind: FuzzKind::Generated,
        });
        assert!(report.ok(), "{}", format_failures(&report));
    }

    #[test]
    fn opt_levels_agree() {
        let report = run_fuzz(&FuzzConfig {
            iters: 20,
            seed: 19,
            kind: FuzzKind::Differential,
        });
        assert!(report.ok(), "{}", format_failures(&report));
    }

    #[test]
    fn mutated_sources_do_not_panic() {
        let report = run_fuzz(&FuzzConfig {
            iters: 25,
            seed: 23,
            kind: FuzzKind::Mutated,
        });
        assert!(report.ok(), "{}", format_failures(&report));
    }

    #[test]
    fn aspect_mutants_typecheck() {
        let report = run_fuzz(&FuzzConfig {
            iters: 16,
            seed: 31,
            kind: FuzzKind::Aspect,
        });
        assert!(report.ok(), "{}", format_failures(&report));
    }

    #[test]
    fn mir_programs_agree_across_opt() {
        let report = run_fuzz(&FuzzConfig {
            iters: 16,
            seed: 37,
            kind: FuzzKind::Mir,
        });
        assert!(report.ok(), "{}", format_failures(&report));
    }

    #[test]
    fn structural_mutants_do_not_panic() {
        let report = run_fuzz(&FuzzConfig {
            iters: 20,
            seed: 29,
            kind: FuzzKind::Structural,
        });
        assert!(report.ok(), "{}", format_failures(&report));
    }

    #[test]
    fn format_tape_does_not_panic() {
        let report = run_fuzz(&FuzzConfig {
            iters: 20,
            seed: 43,
            kind: FuzzKind::Format,
        });
        assert!(report.ok(), "{}", format_failures(&report));
    }

    #[test]
    fn greybox_campaign_is_clean() {
        let report = run_fuzz(&FuzzConfig {
            iters: 16,
            seed: 41,
            kind: FuzzKind::Greybox,
        });
        assert!(report.ok(), "{}", format_failures(&report));
        assert!(report.extra.contains("edges="));
    }

    #[test]
    fn seed_is_reproducible() {
        let cfg = FuzzConfig {
            iters: 8,
            seed: 99,
            kind: FuzzKind::Generated,
        };
        let a = run_fuzz(&cfg);
        let b = run_fuzz(&cfg);
        assert_eq!(a.passed, b.passed);
        assert_eq!(a.failed, b.failed);
    }

    fn format_failures(r: &FuzzReport) -> String {
        let mut s = r.summary();
        for f in &r.failures {
            s.push_str(&format!(
                "\n[{}] seed={} case={}\n{}\n---\n{}\n",
                f.property, f.seed, f.case, f.detail, f.source
            ));
        }
        s
    }
}

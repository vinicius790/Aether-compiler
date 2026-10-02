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
//!    do not panic, at `-O0` and at `-O2`.
//! 6. **Aggregate synthesis** — struct / array / `i64` / `f64` / `char` /
//!    `string` programs compile without diagnostics and agree across
//!    optimisation levels.
//! 7. **Language synthesis** (`lang`) — enums, `match`, `if let`, tuples,
//!    compound assignment, bit operators, built-ins, `yield`, by-value
//!    aggregates and multi-file programs; checked by the full oracle suite
//!    below.
//!
//! Oracle suite for every synthesized program (`check_program`): compiles at
//! `-O0`/`-O1`/`-O2`; `verify_module` on the IR of each level; value and
//! stdout equal across the three levels; two runs of the same compiled
//! program are identical (determinism); `Vm::run_budget` with budgets 1, 7
//! and 1000 gives the same value, stdout and total steps as `run()`;
//! `aether fmt` output (single-file programs) recompiles to the same stdout
//! and is a fixed point of the formatter; a program that prints `VIOLATION`
//! caught an aggregate aliasing its copy.
//!
//! Every case runs on a watchdog thread: a case that takes longer than 5 s
//! (`AETHER_FUZZ_TIMEOUT_MS` overrides) is a failure (`timeout`), which is
//! how compiler hangs on hostile input are caught.

mod agg;
mod format;
mod gen;
mod greybox;
mod lang;
mod mir;
mod mutate;
mod rng;
mod sketch;

pub use agg::gen_aggregate_program;
pub use gen::gen_program;
pub use greybox::{run_greybox, GreyboxReport};
pub use lang::{gen_lang_program, gen_lang_source, LangProgram, ALLOW_MATCH_EXPR};
pub use mir::{check_ir, decoy_stats, gen_ir};
pub use mutate::{crossover, mutate_structural, shrink};
pub use rng::FuzzRng;
pub use sketch::{crossover_sketch, gen_sketch, mutate_aspect, ProgramSketch};

use crate::driver::{compile_files, compile_source, run_compiled, CompileOptions, Compiled};
use crate::lexer::tokenize;
use crate::parser::parse;
use crate::span::FileId;
use crate::token::TokenKind;
use crate::vm::{Step, Value, Vm, VmOptions};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

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
    Aggregate,
    Lang,
}

impl FuzzKind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "all" => FuzzKind::All,
            "agg" | "aggregate" => FuzzKind::Aggregate,
            "lang" | "language" => FuzzKind::Lang,
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
                FuzzKind::Aggregate,
                FuzzKind::Lang,
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

/// Knobs that are not part of [`FuzzConfig`] (kept source-compatible).
#[derive(Debug, Clone, Copy)]
pub struct FuzzOptions {
    /// Watchdog limit per case; a case that takes longer is a `timeout`
    /// failure.
    pub timeout: Duration,
    /// Run the `aether fmt` round-trip oracle on single-file programs.
    pub fmt: bool,
}

impl FuzzOptions {
    /// Defaults, overridable with `AETHER_FUZZ_TIMEOUT_MS` (default 5000)
    /// and `AETHER_FUZZ_NO_FMT=1`.
    pub fn from_env() -> Self {
        let ms = std::env::var("AETHER_FUZZ_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(5000);
        FuzzOptions {
            timeout: Duration::from_millis(ms),
            fmt: std::env::var_os("AETHER_FUZZ_NO_FMT").is_none(),
        }
    }
}

pub fn run_fuzz(cfg: &FuzzConfig) -> FuzzReport {
    run_fuzz_with(cfg, FuzzOptions::from_env())
}

pub fn run_fuzz_with(cfg: &FuzzConfig, opts: FuzzOptions) -> FuzzReport {
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
            match run_case(kind, case_seed, opts) {
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

/// Replay one case of `kind` from the case seed a failure report printed.
pub fn replay_case(kind: FuzzKind, case_seed: u64) -> Result<(), FuzzFailure> {
    run_case(kind, case_seed, FuzzOptions::from_env())
}

/// Shared slot where a property records its current input, so the watchdog
/// can report it when the case never returns.
#[derive(Clone)]
struct Cx {
    input: Arc<Mutex<String>>,
    fmt: bool,
    /// Set by a failing property: a single-file program the driver loop
    /// minimizes after the watchdog thread has returned (so minimization
    /// time never counts against the case timeout).
    shrink: Arc<Mutex<Option<ShrinkJob>>>,
}

#[derive(Clone)]
struct ShrinkJob {
    src: String,
    label: &'static str,
    prefix: &'static str,
    strict: bool,
    fmt: bool,
    property: &'static str,
}

impl Cx {
    fn set(&self, src: &str) {
        if let Ok(mut g) = self.input.lock() {
            g.clear();
            g.push_str(src);
        }
    }

    fn get(&self) -> String {
        self.input.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

fn dispatch(kind: FuzzKind, rng: &mut FuzzRng, cx: &Cx) -> Result<(), FuzzFailure> {
    match kind {
        FuzzKind::Lexer => prop_lexer(rng, cx),
        FuzzKind::Parser => prop_parser(rng, cx),
        FuzzKind::Pipeline => prop_pipeline(rng, cx),
        FuzzKind::Generated => prop_generated(rng, cx),
        FuzzKind::Differential => prop_differential(rng, cx),
        FuzzKind::Mutated => prop_mutated(rng, cx),
        FuzzKind::Structural => prop_structural(rng, cx),
        FuzzKind::Aspect => prop_aspect(rng, cx),
        FuzzKind::Mir => prop_mir(rng, cx),
        FuzzKind::Format => prop_format(rng, cx),
        FuzzKind::Aggregate => prop_aggregate(rng, cx),
        FuzzKind::Lang => prop_lang(rng, cx),
        FuzzKind::Greybox | FuzzKind::All => unreachable!(),
    }
}

/// Run one case on its own thread (large stack: the parser and sema recurse
/// to depth 256) and give up on it after `timeout`.
fn run_case(kind: FuzzKind, case_seed: u64, opts: FuzzOptions) -> Result<(), FuzzFailure> {
    let timeout = opts.timeout;
    let cx = Cx {
        input: Arc::new(Mutex::new(String::new())),
        fmt: opts.fmt,
        shrink: Arc::new(Mutex::new(None)),
    };
    let cx2 = cx.clone();
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("aether-fuzz-case".into())
        .stack_size(256 << 20)
        .spawn(move || {
            let mut rng = FuzzRng::new(case_seed);
            let res = dispatch(kind, &mut rng, &cx2);
            let _ = tx.send(res);
        });
    let handle = match spawned {
        Ok(h) => h,
        Err(e) => return Err(fail("case_thread", format!("cannot spawn: {e}"), "")),
    };
    match rx.recv_timeout(timeout) {
        Ok(res) => {
            let _ = handle.join();
            match res {
                Err(f) => {
                    let job = cx.shrink.lock().ok().and_then(|g| g.clone());
                    Err(match job {
                        Some(j) if j.property == f.property => minimize(&j, f),
                        _ => f,
                    })
                }
                ok => ok,
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout) => Err(fail(
            "timeout",
            format!(
                "case ({kind:?}) still running after {} ms: compiler or VM hang",
                timeout.as_millis()
            ),
            &cx.get(),
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(fail(
            "case_thread",
            "case thread died without a result (panic outside catch_unwind or stack overflow)",
            &cx.get(),
        )),
    }
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
        FuzzKind::Aggregate => 0xCC,
        FuzzKind::Lang => 0xDD,
    }
}

/// Compile hostile input at `-O0` and `-O2`; `Err(level)` names the level
/// that panicked. The optimizer only ever sees well-typed IR, so junk that
/// fails sema exercises the same path at both levels, but anything that
/// slips through must survive the full pass pipeline too.
fn compile_both_levels(name: &str, src: &str) -> Result<(), u8> {
    for level in [0u8, 2] {
        let opts = CompileOptions {
            opt_level: level,
            color: false,
        };
        let caught = catch_unwind(AssertUnwindSafe(|| {
            let _ = tokenize(FileId(0), src);
            let _ = compile_source(name, src, &opts);
        }));
        if caught.is_err() {
            return Err(level);
        }
    }
    Ok(())
}

fn prop_format(rng: &mut FuzzRng, cx: &Cx) -> Result<(), FuzzFailure> {
    let src = format::gen_format(rng);
    cx.set(&src);
    match compile_both_levels("<fmt>", &src) {
        Ok(()) => Ok(()),
        Err(level) => Err(fail(
            "format_no_panic",
            format!("grammar-tape input panicked the compiler at -O{level}"),
            &src,
        )),
    }
}

// ---------------------------------------------------------------------------
// Oracle suite shared by the synthesizing properties
// ---------------------------------------------------------------------------

/// A program under test: one in-memory source, or several files written to a
/// private temporary directory (removed on drop) for `use` imports.
struct Prog {
    files: Vec<(String, String)>,
    dir: Option<std::path::PathBuf>,
    label: &'static str,
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl Prog {
    fn single(label: &'static str, src: &str) -> Prog {
        Prog {
            files: vec![("main.ae".into(), src.to_string())],
            dir: None,
            label,
        }
    }

    fn from_lang(label: &'static str, p: LangProgram) -> Result<Prog, String> {
        if p.files.len() == 1 {
            return Ok(Prog {
                files: p.files,
                dir: None,
                label,
            });
        }
        let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aether_fuzz_{}_{n}", std::process::id()));
        let prog = Prog {
            files: p.files,
            dir: Some(dir.clone()),
            label,
        };
        for (name, text) in &prog.files {
            let path = dir.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {parent:?}: {e}"))?;
            }
            std::fs::write(&path, text).map_err(|e| format!("write {path:?}: {e}"))?;
        }
        Ok(prog)
    }

    fn text(&self) -> String {
        if self.files.len() == 1 {
            return self.files[0].1.clone();
        }
        let mut s = String::new();
        for (n, t) in &self.files {
            s.push_str(&format!("// ==== {n} ====\n{t}\n"));
        }
        s
    }

    fn compile(&self, level: u8) -> Result<Compiled, String> {
        let opts = CompileOptions {
            opt_level: level,
            color: false,
        };
        match &self.dir {
            None => Ok(compile_source(self.label, &self.files[0].1, &opts)),
            Some(d) => {
                let main = d.join(&self.files[0].0);
                compile_files(&main.to_string_lossy(), &[], &opts)
            }
        }
    }
}

impl Drop for Prog {
    fn drop(&mut self) {
        if let Some(d) = &self.dir {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

struct Oracle {
    prefix: &'static str,
    /// Generated programs must not fail at run time. When false, a failure
    /// that is the same (step limit, division by zero, ...) at every level
    /// is accepted.
    strict: bool,
    /// Check the formatter (`aether fmt`) on single-file programs.
    fmt: bool,
}

fn pfail(o: &Oracle, id: &str, detail: impl Into<String>, prog: &Prog) -> FuzzFailure {
    // Leaked on failure only: the property name is composed per kind.
    let name: &'static str = Box::leak(format!("{}_{id}", o.prefix).into_boxed_str());
    fail(name, detail, &prog.text())
}

#[derive(Debug, PartialEq)]
struct Outcome {
    value: Value,
    stdout: String,
    steps: u64,
}

fn run_once(c: &mut Compiled) -> Result<Outcome, String> {
    match catch_unwind(AssertUnwindSafe(|| run_compiled(c))) {
        Ok(Ok((value, stdout, steps))) => Ok(Outcome { value, stdout, steps }),
        Ok(Err(e)) => Err(format!("{}", e)),
        Err(_) => Err("panic while running".into()),
    }
}

struct Capture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Capture {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Run to completion in slices of `budget` steps (resuming on `Yielded`).
fn run_budgeted(c: &Compiled, budget: u64) -> Result<Outcome, String> {
    let Some(bc) = c.bytecode.as_ref() else {
        return Err("no bytecode".into());
    };
    let slot = Arc::new(Mutex::new(Vec::new()));
    let r = catch_unwind(AssertUnwindSafe(|| {
        let mut vm = Vm::new(bc, VmOptions::default()).with_stdout(Box::new(Capture(slot.clone())));
        let value = loop {
            match vm.run_budget(budget) {
                Ok(Step::Finished(v)) => break Ok(v),
                Ok(Step::Yielded) => {}
                Err(e) => break Err(e.to_string()),
            }
        };
        (value, vm.steps())
    }));
    let stdout = String::from_utf8_lossy(&slot.lock().unwrap()).into_owned();
    match r {
        Ok((Ok(value), steps)) => Ok(Outcome { value, stdout, steps }),
        Ok((Err(e), _)) => Err(e),
        Err(_) => Err("panic while running".into()),
    }
}

fn error_class(s: &str) -> &'static str {
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
}

const LEVELS: [u8; 3] = [0, 1, 2];

/// Full oracle suite (see the module docs) on one program.
fn check_program(prog: &Prog, o: &Oracle) -> Result<(), FuzzFailure> {
    // compile at every level
    let mut compiled: Vec<Compiled> = Vec::new();
    for level in LEVELS {
        let c = match catch_unwind(AssertUnwindSafe(|| prog.compile(level))) {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => return Err(pfail(o, "compile", format!("-O{level}: {e}"), prog)),
            Err(_) => {
                return Err(pfail(
                    o,
                    "compile",
                    format!("compiler panicked at -O{level}"),
                    prog,
                ))
            }
        };
        if c.diags.has_errors() {
            return Err(pfail(
                o,
                "well_typed",
                format!(
                    "rejected at -O{level}:\n{}",
                    c.diags.render(&c.session, false)
                ),
                prog,
            ));
        }
        compiled.push(c);
    }
    // IR well-formedness after every pipeline
    for (c, level) in compiled.iter().zip(LEVELS) {
        if let Some(m) = &c.ir {
            if let Err(e) = crate::ir::verify::verify_module(m) {
                return Err(pfail(o, "ir_verify", format!("-O{level}: {e}"), prog));
            }
        }
    }
    // run
    let mut outs: Vec<Result<Outcome, String>> = Vec::new();
    for c in compiled.iter_mut() {
        outs.push(run_once(c));
    }
    if outs.iter().any(|r| r.is_err()) {
        let msgs: Vec<String> = outs
            .iter()
            .zip(LEVELS)
            .map(|(r, l)| match r {
                Ok(_) => format!("-O{l}: ok"),
                Err(e) => format!("-O{l}: {e}"),
            })
            .collect();
        let all_err = outs.iter().all(|r| r.is_err());
        let classes: Vec<&str> = outs
            .iter()
            .filter_map(|r| r.as_ref().err().map(|e| error_class(e)))
            .collect();
        if !o.strict && all_err && classes.windows(2).all(|w| w[0] == w[1]) {
            return Ok(());
        }
        let id = if all_err { "runtime_error" } else { "opt_equiv" };
        return Err(pfail(o, id, msgs.join("\n"), prog));
    }
    let outs: Vec<Outcome> = outs.into_iter().map(|r| r.unwrap()).collect();
    for k in 1..outs.len() {
        if outs[0].value != outs[k].value {
            return Err(pfail(
                o,
                "opt_equiv",
                format!(
                    "-O0 returned {:?}, -O{} returned {:?}",
                    outs[0].value, LEVELS[k], outs[k].value
                ),
                prog,
            ));
        }
        if outs[0].stdout != outs[k].stdout {
            return Err(pfail(
                o,
                "opt_equiv",
                format!(
                    "stdout differs between -O0 and -O{}:\n{}",
                    LEVELS[k],
                    first_diff(&outs[0].stdout, &outs[k].stdout)
                ),
                prog,
            ));
        }
    }
    if outs[0].stdout.contains("VIOLATION") {
        return Err(pfail(
            o,
            "value_semantics",
            "program printed VIOLATION: an aggregate copy aliased its source",
            prog,
        ));
    }
    // determinism and budgeted execution at the extreme levels
    for idx in [0usize, 2] {
        let level = LEVELS[idx];
        let again = match run_once(&mut compiled[idx]) {
            Ok(a) => a,
            Err(e) => return Err(pfail(o, "determinism", format!("-O{level} rerun failed: {e}"), prog)),
        };
        if again != outs[idx] {
            return Err(pfail(
                o,
                "determinism",
                format!(
                    "-O{level}: second run differs ({:?} vs {:?}, steps {} vs {})",
                    outs[idx].value, again.value, outs[idx].steps, again.steps
                ),
                prog,
            ));
        }
        for budget in [1u64, 7, 1000] {
            if budget == 1 && outs[idx].steps > 3_000_000 {
                continue;
            }
            let b = match run_budgeted(&compiled[idx], budget) {
                Ok(b) => b,
                Err(e) => {
                    return Err(pfail(
                        o,
                        "budget_equiv",
                        format!("-O{level} budget {budget}: {e}"),
                        prog,
                    ))
                }
            };
            if b != outs[idx] {
                return Err(pfail(
                    o,
                    "budget_equiv",
                    format!(
                        "-O{level} budget {budget}: value {:?} vs {:?}, steps {} vs {}, stdout {}",
                        b.value,
                        outs[idx].value,
                        b.steps,
                        outs[idx].steps,
                        if b.stdout == outs[idx].stdout { "same" } else { "differs" }
                    ),
                    prog,
                ));
            }
        }
    }
    if o.fmt && prog.dir.is_none() {
        check_fmt(prog, o, &outs[0])?;
    }
    Ok(())
}

/// `aether fmt` must preserve behaviour and be a fixed point.
fn check_fmt(prog: &Prog, o: &Oracle, original: &Outcome) -> Result<(), FuzzFailure> {
    let src = &prog.files[0].1;
    let opts = CompileOptions {
        opt_level: 0,
        color: false,
    };
    let formatted = match catch_unwind(AssertUnwindSafe(|| {
        crate::pretty::pretty_program(&compile_source("<fmt>", src, &opts).program)
    })) {
        Ok(t) => t,
        Err(_) => return Err(pfail(o, "fmt_panic", "pretty printer panicked", prog)),
    };
    let mut c = match catch_unwind(AssertUnwindSafe(|| compile_source("<fmt>", &formatted, &opts))) {
        Ok(c) => c,
        Err(_) => return Err(pfail(o, "fmt_panic", "compiling formatted output panicked", prog)),
    };
    if c.diags.has_errors() {
        return Err(pfail(
            o,
            "fmt_idempotent",
            format!(
                "formatted output does not compile:\n{}\n--- formatted ---\n{formatted}",
                c.diags.render(&c.session, false)
            ),
            prog,
        ));
    }
    match run_once(&mut c) {
        Ok(out) => {
            if out.value != original.value || out.stdout != original.stdout {
                return Err(pfail(
                    o,
                    "fmt_idempotent",
                    format!(
                        "formatted program behaves differently: value {:?} vs {:?}\n{}\n--- formatted ---\n{formatted}",
                        original.value,
                        out.value,
                        first_diff(&original.stdout, &out.stdout)
                    ),
                    prog,
                ));
            }
        }
        Err(e) => {
            return Err(pfail(
                o,
                "fmt_idempotent",
                format!("formatted program fails at run time: {e}\n--- formatted ---\n{formatted}"),
                prog,
            ))
        }
    }
    let again = crate::pretty::pretty_program(&c.program);
    if again != formatted {
        return Err(pfail(
            o,
            "fmt_fixpoint",
            format!("fmt(fmt(p)) != fmt(p)\n--- first ---\n{formatted}\n--- second ---\n{again}"),
            prog,
        ));
    }
    Ok(())
}

fn first_diff(a: &str, b: &str) -> String {
    for (i, (la, lb)) in a.lines().zip(b.lines()).enumerate() {
        if la != lb {
            return format!("first difference at stdout line {}: {la:?} vs {lb:?}", i + 1);
        }
    }
    format!("line counts differ: {} vs {}", a.lines().count(), b.lines().count())
}

/// Line-chunk delta debugging bounded by an attempt count. `pred` is true
/// while the candidate still fails the same way.
fn shrink_lines(src: &str, mut pred: impl FnMut(&str) -> bool) -> String {
    let mut lines: Vec<String> = src.lines().map(|l| l.to_string()).collect();
    let start = std::time::Instant::now();
    let mut attempts = 0;
    let mut chunk = (lines.len() / 2).max(1);
    loop {
        let mut i = 0;
        let mut progress = false;
        while i < lines.len() {
            if attempts >= 600 || start.elapsed() > Duration::from_secs(20) {
                return lines.join("\n");
            }
            let end = (i + chunk).min(lines.len());
            let mut cand = lines.clone();
            cand.drain(i..end);
            attempts += 1;
            if pred(&cand.join("\n")) {
                lines = cand;
                progress = true;
            } else {
                i = end;
            }
        }
        if chunk == 1 && !progress {
            break;
        }
        if chunk > 1 {
            chunk = (chunk / 2).max(1);
        }
    }
    lines.join("\n")
}

/// Run the oracle suite; a single-file failure records a shrink job.
fn check_and_minimize(prog: &Prog, o: &Oracle, cx: &Cx) -> Result<(), FuzzFailure> {
    let Err(f) = check_program(prog, o) else {
        return Ok(());
    };
    let p = f.property;
    if prog.dir.is_none()
        && !p.ends_with("_compile")
        && !p.ends_with("_well_typed")
        && !p.ends_with("_runtime_error")
    {
        if let Ok(mut g) = cx.shrink.lock() {
            *g = Some(ShrinkJob {
                src: prog.files[0].1.clone(),
                label: prog.label,
                prefix: o.prefix,
                strict: o.strict,
                fmt: o.fmt,
                property: p,
            });
        }
    }
    Err(f)
}

/// Minimize on its own watchdog thread (30 s); falls back to the original.
fn minimize(job: &ShrinkJob, f: FuzzFailure) -> FuzzFailure {
    let j = job.clone();
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(move || {
            let o = Oracle {
                prefix: j.prefix,
                strict: j.strict,
                fmt: j.fmt,
            };
            let shrunk = shrink_lines(&j.src, |cand| {
                let p = Prog::single(j.label, cand);
                matches!(check_program(&p, &o), Err(e) if e.property == j.property)
            });
            let p = Prog::single(j.label, &shrunk);
            let r = check_program(&p, &o).err().map(|mut e| {
                e.detail = format!(
                    "{}\n(minimized from {} to {} bytes)",
                    e.detail,
                    j.src.len(),
                    shrunk.len()
                );
                e
            });
            let _ = tx.send(r);
        });
    if spawned.is_err() {
        return f;
    }
    match rx.recv_timeout(Duration::from_secs(30)) {
        Ok(Some(m)) if m.source.len() < f.source.len() => m,
        _ => f,
    }
}

fn prop_aggregate(rng: &mut FuzzRng, cx: &Cx) -> Result<(), FuzzFailure> {
    let src = gen_aggregate_program(rng);
    cx.set(&src);
    let o = Oracle {
        prefix: "agg",
        strict: true,
        fmt: cx.fmt,
    };
    check_and_minimize(&Prog::single("<agg>", &src), &o, cx)
}

fn prop_differential(rng: &mut FuzzRng, cx: &Cx) -> Result<(), FuzzFailure> {
    let src = gen_program(rng);
    cx.set(&src);
    let o = Oracle {
        prefix: "diff",
        strict: false,
        fmt: cx.fmt,
    };
    check_and_minimize(&Prog::single("<diff>", &src), &o, cx)
}

fn prop_lang(rng: &mut FuzzRng, cx: &Cx) -> Result<(), FuzzFailure> {
    let p = gen_lang_program(rng);
    let text: String = p
        .files
        .iter()
        .map(|(n, t)| format!("// ==== {n} ====\n{t}\n"))
        .collect();
    cx.set(&text);
    let prog = match Prog::from_lang("<lang>", p) {
        Ok(p) => p,
        Err(e) => return Err(fail("lang_setup", e, &text)),
    };
    let o = Oracle {
        prefix: "lang",
        strict: true,
        fmt: cx.fmt,
    };
    check_and_minimize(&prog, &o, cx)
}

fn prop_lexer(rng: &mut FuzzRng, cx: &Cx) -> Result<(), FuzzFailure> {
    let src = random_source(rng);
    cx.set(&src);
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

fn prop_parser(rng: &mut FuzzRng, cx: &Cx) -> Result<(), FuzzFailure> {
    let src = random_source(rng);
    cx.set(&src);
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

fn prop_pipeline(rng: &mut FuzzRng, cx: &Cx) -> Result<(), FuzzFailure> {
    let src = random_source(rng);
    cx.set(&src);
    let opts = CompileOptions {
        opt_level: 2,
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

fn prop_generated(rng: &mut FuzzRng, cx: &Cx) -> Result<(), FuzzFailure> {
    let src = gen_program(rng);
    cx.set(&src);
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

fn prop_structural(rng: &mut FuzzRng, cx: &Cx) -> Result<(), FuzzFailure> {
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
    cx.set(&src);
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

fn prop_aspect(rng: &mut FuzzRng, cx: &Cx) -> Result<(), FuzzFailure> {
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
    cx.set(&src);
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

fn prop_mir(rng: &mut FuzzRng, _cx: &Cx) -> Result<(), FuzzFailure> {
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

fn prop_mutated(rng: &mut FuzzRng, cx: &Cx) -> Result<(), FuzzFailure> {
    let base = match rng.int(0, 2) {
        0 => gen_program(rng),
        1 => gen_aggregate_program(rng),
        _ => gen_lang_source(rng),
    };
    let src = if rng.bool() {
        mutate_source(rng, &base)
    } else {
        token_mutate(rng, &base)
    };
    cx.set(&src);
    match compile_both_levels("<mut>", &src) {
        Ok(()) => Ok(()),
        Err(level) => Err(fail(
            "mut_no_panic",
            format!("mutated source panicked the compiler at -O{level}"),
            &src,
        )),
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
    match rng.int(0, 9) {
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
        5 => {
            let base = gen_lang_source(rng);
            token_mutate(rng, &base)
        }
        6 => {
            let base = gen_aggregate_program(rng);
            token_mutate(rng, &base)
        }
        7 => {
            let base = gen_lang_source(rng);
            mutate_source(rng, &base)
        }
        _ => {
            let mut s = gen_program(rng);
            let n = rng.int(0, 16) as usize;
            s.push_str(&rng.random_ascii(n));
            s
        }
    }
}

/// Token-level mutation of a valid program: swap, delete, duplicate,
/// replace or insert tokens (new-syntax tokens are over-represented).
/// Whitespace and comments are normalised to single spaces.
fn token_mutate(rng: &mut FuzzRng, src: &str) -> String {
    let (tokens, _) = tokenize(FileId(0), src);
    let mut toks: Vec<String> = tokens
        .iter()
        .filter(|t| t.kind != TokenKind::Eof)
        .filter_map(|t| src.get(t.span.start.0 as usize..t.span.end.0 as usize))
        .map(|s| s.to_string())
        .collect();
    if toks.is_empty() {
        return keyword_soup(rng);
    }
    let n = rng.int(1, 4);
    for _ in 0..n {
        let len = toks.len() as i32;
        if len == 0 {
            break;
        }
        let i = rng.int(0, len - 1) as usize;
        match rng.int(0, 5) {
            0 => {
                let j = rng.int(0, len - 1) as usize;
                toks.swap(i, j);
            }
            1 => {
                toks.remove(i);
            }
            2 => {
                let t = toks[i].clone();
                toks.insert(i, t);
            }
            3 => toks[i] = (*rng.choose(VOCAB)).to_string(),
            4 => toks.insert(i, (*rng.choose(VOCAB)).to_string()),
            _ => {
                // move a token somewhere else
                let t = toks.remove(i);
                let j = rng.int(0, toks.len() as i32) as usize;
                toks.insert(j, t);
            }
        }
    }
    toks.join(" ")
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

/// Tokens of the whole language, new 0.3 syntax included.
const VOCAB: &[&str] = &[
    "fn", "let", "mut", "if", "else", "while", "for", "in", "return", "struct", "true", "false",
    "as", "(", ")", "{", "}", "[", "]", ";", ":", "->", "..", "+", "-", "*", "/", "==", "!=", "<",
    ">", "&&", "||", "main", "i32", "0", "1", "\"x\"", "enum", "match", "::", "=>", "use", "pub",
    "yield", "<<=", ">>=", "+=", "-=", "*=", "&=", "|=", "^=", "/=", "%=", "<<", ">>", "&", "|",
    "^", "%", "!", "_", ",", "=", ".", ".0", ".1", "0x", "0xFF", "0b101", "0b", "0o17", "1_000",
    "0x_", "1__0", "\"\\u{41}\"", "'\\u{'", "'\\u{110000}'", "\"\\u{\"", "\"\\u{zz}\"",
    "if let", "E::A", "E::B(1)", "E::B(x, _)", "(a, b)", "let (a, b) = t;", "t.0.1", "abs", "min",
    "max", "clamp", "pow_i32", "sqrt", "to_string", "len", "i64", "f64", "char", "string", "bool",
    "'a'", "3.0", "1_0.5", "\"use\"", "use \"x.ae\";", "pub fn", "pub struct", "pub enum",
];

fn keyword_soup(rng: &mut FuzzRng) -> String {
    let n = rng.int(3, 28) as usize;
    let mut s = String::new();
    for i in 0..n {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(*rng.choose(VOCAB));
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
    // unterminated block followed by the next item (used to spin forever)
    "fn h0(p0: i32) -> i32 { return 8;\nfn main() -> i32 { return h0(1); }",
    "fn main() -> i32 { struct S { x: i32 }",
    // new syntax: enums, match, tuples, use, escapes, compound operators
    "enum",
    "enum E",
    "enum E {",
    "enum E { A(",
    "enum E { A(i32,) }",
    "enum E { A, A }",
    "enum E { A(E) }",
    "enum E { A(B), } enum B { C(E) }",
    "enum E { A } fn main() -> i32 { match E::A { } return 0; }",
    "enum E { A } fn main() -> i32 { match E::A { E::A => } return 0; }",
    "enum E { A } fn main() -> i32 { match E::A { E::A => { return 1; } E::A => { return 2; } } }",
    "enum E { A(i32) } fn main() -> i32 { match E::A(1) { E::A(x, y) => { return x; } } return 0; }",
    "enum E { A(i32) } fn main() -> i32 { if let E::A(x) = E::A(1) { return x; } else { return 0; } }",
    "enum E { A } fn main() -> i32 { if let E::B = E::A { } return 0; }",
    "fn main() -> i32 { match 1 { 1 => { } } return 0; }",
    "fn main() -> i32 { match 1 { _ => { } _ => { } } return 0; }",
    "fn main() -> i32 { match 1 { x => { return x; } } }",
    "fn main() -> i32 { let (a, b) = (1, 2, 3); return a; }",
    "fn main() -> i32 { let (a, b) = 1; return a; }",
    "fn main() -> i32 { let t = (1, 2); return t.2; }",
    "fn main() -> i32 { let t = (1, 2); t.5 = 1; return t.0.0; }",
    "fn main() -> i32 { let t = ((1, 2), 3); return t.0.1; }",
    "fn main() -> i32 { let t = (); return 0; }",
    "fn main() -> i32 { let mut x = 1; x <<= ; return x; }",
    "fn main() -> i32 { let mut x = 1; x <<= 70; x >>= -1; x **= 2; return x; }",
    "fn main() -> i32 { let x = 0x; return 0; }",
    "fn main() -> i32 { let x = 0b2; return 0; }",
    "fn main() -> i32 { let x = 0xFFFF_FFFF_FFFF_FFFF_F; return 0; }",
    "fn main() -> i32 { let x = 1__0_; return x; }",
    "fn main() -> i32 { let c = '\\u{'; return 0; }",
    "fn main() -> i32 { let c = '\\u{110000}'; return 0; }",
    "fn main() -> i32 { let c = '\\u{D800}'; return 0; }",
    "fn main() -> i32 { let s = \"\\u{}\"; return 0; }",
    "fn main() -> i32 { let s = \"\\u{1234567}\"; return 0; }",
    "use",
    "use \"",
    "use \"x\"",
    "use \"does_not_exist.ae\"; fn main() -> i32 { return 0; }",
    "use 1; fn main() -> i32 { return 0; }",
    "pub",
    "pub pub fn main() -> i32 { return 0; }",
    "pub use \"x.ae\";",
    "pub let x = 1;",
    "fn main() -> i32 { yield; yield yield; return 0; }",
    "fn main() -> i32 { yield 1; return 0; }",
    "fn main() -> i32 { return abs(); }",
    "fn main() -> i32 { return clamp(1, 2); }",
    "fn main() -> i32 { return pow_i32(2, 100000); }",
    "fn main() -> i32 { return a::b::c; }",
    "fn main() -> i32 { return ::; }",
    "fn main() -> i32 { x => y; }",
    "fn main() -> i32 { return !!!1 <<= 2; }",
    "fn main() -> i32 { return 1 << 2 << 3 >> 4 & 5 | 6 ^ 7; }",
];

fn corpus_case(i: usize) -> &'static str {
    CORPUS[i % CORPUS.len()]
}

fn fail(property: &'static str, detail: impl Into<String>, source: &str) -> FuzzFailure {
    let mut src = source.to_string();
    if src.len() > 16_000 {
        let mut cut = 16_000;
        while !src.is_char_boundary(cut) {
            cut -= 1;
        }
        src.truncate(cut);
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

    /// Campaigns in unit tests skip the `fmt` round trip (known pretty-printer
    /// bugs, docs/fuzzing.md) and use a generous watchdog.
    fn run_fuzz(cfg: &FuzzConfig) -> FuzzReport {
        run_fuzz_with(
            cfg,
            FuzzOptions {
                timeout: Duration::from_secs(120),
                fmt: false,
            },
        )
    }

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

    fn compiles_clean(label: &'static str, p: LangProgram) -> Result<(), String> {
        let prog = Prog::from_lang(label, p)?;
        for level in [0u8, 2] {
            let c = prog.compile(level)?;
            if c.diags.has_errors() {
                return Err(format!(
                    "-O{level}:\n{}\n---\n{}",
                    c.diags.render(&c.session, false),
                    prog.text()
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn aggregate_programs_typecheck() {
        // Well-typedness only: the oracle suite runs in `aether fuzz --kind agg`
        // and in tests/fuzz_smoke.rs.
        let mut rng = FuzzRng::new(53 ^ kind_salt(FuzzKind::Aggregate));
        for _ in 0..40u32 {
            let src = gen_aggregate_program(&mut FuzzRng::new(rng.next_u64()));
            let p = LangProgram {
                files: vec![("main.ae".into(), src)],
            };
            if let Err(e) = compiles_clean("<agg>", p) {
                panic!("{e}");
            }
        }
    }

    #[test]
    fn lang_programs_typecheck() {
        let mut rng = FuzzRng::new(59 ^ kind_salt(FuzzKind::Lang));
        for _ in 0..40u32 {
            let p = gen_lang_program(&mut FuzzRng::new(rng.next_u64()));
            if let Err(e) = compiles_clean("<lang>", p) {
                panic!("{e}");
            }
        }
    }

    #[test]
    fn aggregate_kind_is_in_all_suite_and_parses() {
        assert_eq!(FuzzKind::parse("agg"), Some(FuzzKind::Aggregate));
        assert_eq!(FuzzKind::parse("aggregate"), Some(FuzzKind::Aggregate));
        assert!(FuzzKind::All.suite().contains(&FuzzKind::Aggregate));
        assert_eq!(FuzzKind::parse("lang"), Some(FuzzKind::Lang));
        assert!(FuzzKind::All.suite().contains(&FuzzKind::Lang));
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

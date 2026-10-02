//! Aether command-line interface (no external CLI crate).

use aether::ast::Item;
use aether::driver::{
    compile_file, compile_files, compile_source, compile_sources_public, default_includes, dump_ast,
    dump_bc, dump_ir_text, dump_tokens, ir_inst_count, run_compiled_with,
    CompileOptions, Compiled, LliStatus,
};
use aether::span::{FileId, Session};
use aether::vm::VmOptions;
use std::env;
use std::io::{self, Write};
use std::process::ExitCode;

/// Writes `s` to stdout and flushes. When the reader has gone away (EPIPE,
/// `aether dump-tokens f.ae | head -1`) the process exits quietly with
/// status 0, like most command-line tools; any other write error is reported
/// once and exits 1. Every stdout write of the CLI goes through here, so no
/// `print!` can panic.
fn out(s: &str) {
    let mut o = io::stdout().lock();
    if let Err(e) = o.write_all(s.as_bytes()).and_then(|()| o.flush()) {
        stdout_failed(&e);
    }
}

fn stdout_failed(e: &io::Error) -> ! {
    if e.kind() == io::ErrorKind::BrokenPipe {
        std::process::exit(0);
    }
    eprintln!("aether: cannot write to stdout: {e}");
    std::process::exit(1);
}

macro_rules! outp {
    ($($t:tt)*) => { out(&format!($($t)*)) };
}

macro_rules! outln {
    () => { out("\n") };
    ($($t:tt)*) => {{
        let mut line = format!($($t)*);
        line.push('\n');
        out(&line)
    }};
}

/// Stack size for the worker thread that runs every command: deeply nested
/// programs recurse in the parser, sema and the IR passes, and 64 MiB keeps
/// them off the (small) main-thread stack.
const WORKER_STACK_BYTES: usize = 64 * 1024 * 1024;

fn usage() -> &'static str {
    "Aether compiler, optimizer and VM

USAGE:
    aether <command> [options] [file]

COMMANDS:
    check <file>                  type-check only
    run <file> [-O<n>] [--timings] [--stats] [--backend vm|llvm] [--timeout <s>]
    compile <file> [-O<n>] [--emit ir|bytecode|llvm] [-o <path>]
    dump-tokens <file>
    dump-ast <file>
    dump-ir <file> [-O<n>] [--unopt]
    dump-bytecode <file> [-O<n>]
    disassemble <file> [-O<n>]
    dump-llvm <file> [-O<n>]
    dump-hir <file>               typed HIR
    dump-liveness <file> [-O<n>]  live-in sets per block
    optimize <file> [-O<n>]       pass-by-pass before/after report
    fmt <file>                    pretty-print AST back to source
    cfg <file> [-O<n>]            Graphviz DOT of the IR CFG
    verify <file> [-O<n>]         structural IR verifier
    stats <file> [-O<n>]          opt report + liveness + IR size
    profile <file> [-O<n>]        call counts + execution digest
    digest <file> [-O<n>]         deterministic stdout+value fingerprint
    bench <file> [--n <runs>]     -O0 vs -O2: exec time, VM steps, IR size, speedup
    repl [-O<n>]                  stateful REPL (:items, :reset, :help, :quit)
    benchmark [--n <int>]         built-in fib(n) -O0 vs -O2
    fuzz [--iters N] [--seed N] [--kind all|lexer|parser|pipeline|gen|diff|agg|lang|mut|struct|aspect|mir|greybox|format]
    help                          this text (also -h / --help after any command)
    version                       print the version (also --version, -V)

OPTIONS:
    -O<n>, -O <n>         optimizer level 0..2 (default 2)
    --include <file>, -I <file>, --include=<file>
                          compile <file> together with the main file; repeatable
                          (check/run/compile/dump-*/optimize/verify/cfg/stats/
                          profile/digest/bench). AETHER_INCLUDE=a.ae:b.ae adds
                          default includes.
    --max-steps <n>       VM instruction budget (run/profile/digest/bench/benchmark/repl;
                          default 50000000)
    --max-depth <n>       VM call-depth limit (same commands; default 10000)
    --backend vm|llvm     `run` on the bytecode VM (default) or through LLVM `lli`
    --timeout <s>         wall-clock limit for `run` in seconds (default: none on the
                          VM, 10 with --backend llvm, where --max-steps cannot apply)
    --color, --no-color   force or suppress ANSI colour in diagnostics (default: only
                          when stderr is a terminal and NO_COLOR is unset)
    --timings             per-stage timings on stderr (run)
    --stats               exit value, VM steps and opt report on stderr (run)
    --unopt               dump the IR before optimization (dump-ir)
    --emit ir|bytecode|llvm, -o <path> (or --output <path>)
                          artifact kind and output file (compile)
    --n <int>             runs per level (bench, default 5); fib argument (benchmark, default 20)
    --iters N, --seed N, --kind K
                          fuzz configuration (seed accepts 0x...)

EXIT CODES:
    0 ok, 1 compile error, bad usage or fuzz failure, 2 runtime error
    (a closed stdout pipe, as in `aether run f.ae | head -1`, ends quietly with 0)
"
}

struct Args {
    cmd: String,
    file: Option<String>,
    opt: u8,
    timings: bool,
    stats: bool,
    unopt: bool,
    emit: String,
    output: Option<String>,
    n: Option<i32>,
    color: bool,
    iters: u32,
    seed: u64,
    kind: String,
    includes: Vec<String>,
    max_steps: Option<u64>,
    max_depth: Option<usize>,
    /// Wall-clock limit in seconds for `run` (`--timeout`).
    timeout: Option<f64>,
    backend: String,
}

fn parse_args() -> Result<Args, String> {
    let mut raw: Vec<String> = env::args().skip(1).collect();
    if raw.is_empty() {
        // no command at all: a usage error (help text on stderr, exit 1)
        return Err(format!("missing command\n{}", usage()));
    }
    let cmd = raw.remove(0);
    let mut a = Args {
        cmd,
        file: None,
        opt: 2,
        timings: false,
        stats: false,
        unopt: false,
        emit: "bytecode".into(),
        output: None,
        n: None,
        color: false,
        iters: 200,
        seed: 0xA37E400,
        kind: "all".into(),
        includes: Vec::new(),
        max_steps: None,
        max_depth: None,
        timeout: None,
        backend: "vm".into(),
    };
    let mut color_mode: Option<bool> = None;
    let mut i = 0;
    // the operand of option `$name`, or an error naming it
    macro_rules! operand {
        ($name:expr) => {{
            i += 1;
            raw.get(i)
                .cloned()
                .ok_or_else(|| format!("{} needs a value", $name))?
        }};
    }
    while i < raw.len() {
        let s = raw[i].clone();
        let s = s.as_str();
        if s == "-h" || s == "--help" {
            // `aether <cmd> --help` asks for help: the dispatcher prints it
            a.cmd = "help".into();
        } else if s.starts_with("-O") {
            let v = if s == "-O" {
                operand!("-O")
            } else {
                s[2..].to_string()
            };
            a.opt = match v.as_str() {
                "0" => 0,
                "1" => 1,
                "2" => 2,
                _ => return Err(format!("invalid optimization level `{v}` (use -O0, -O1 or -O2)")),
            };
        } else if s == "--timings" {
            a.timings = true;
        } else if s == "--stats" {
            a.stats = true;
        } else if s == "--unopt" {
            a.unopt = true;
        } else if s == "--color" {
            color_mode = Some(true);
        } else if s == "--no-color" {
            color_mode = Some(false);
        } else if s == "--emit" {
            a.emit = operand!("--emit");
            if !matches!(a.emit.as_str(), "ir" | "bytecode" | "llvm") {
                return Err(format!("unknown --emit kind `{}` (use ir, bytecode or llvm)", a.emit));
            }
        } else if s == "-o" || s == "--output" {
            a.output = Some(operand!(s));
        } else if s == "--n" {
            let v = operand!("--n");
            a.n = Some(
                v.parse::<i32>()
                    .map_err(|_| format!("--n needs an integer, found `{v}`"))?,
            );
        } else if s == "--iters" {
            let v = operand!("--iters");
            a.iters = v
                .parse()
                .map_err(|_| format!("--iters needs a non-negative integer, found `{v}`"))?;
        } else if s == "--seed" {
            let v = operand!("--seed");
            a.seed = parse_u64(&v)
                .ok_or_else(|| format!("--seed needs an integer (decimal or 0x...), found `{v}`"))?;
        } else if s == "--kind" {
            a.kind = operand!("--kind");
        } else if s == "--include" || s == "-I" {
            i += 1;
            match raw.get(i) {
                Some(p) if !p.starts_with('-') => a.includes.push(p.clone()),
                _ => return Err(format!("{s} needs a file operand")),
            }
        } else if let Some(p) = s.strip_prefix("--include=") {
            if p.is_empty() {
                return Err("--include needs a file operand".to_string());
            }
            a.includes.push(p.to_string());
        } else if s == "--max-steps" {
            i += 1;
            let v = raw.get(i).and_then(|x| parse_u64(x)).filter(|v| *v > 0);
            a.max_steps = Some(v.ok_or_else(|| "--max-steps needs a positive integer".to_string())?);
        } else if s == "--max-depth" {
            i += 1;
            let v = raw.get(i).and_then(|x| x.parse::<usize>().ok()).filter(|v| *v > 0);
            a.max_depth = Some(v.ok_or_else(|| "--max-depth needs a positive integer".to_string())?);
        } else if s == "--timeout" {
            i += 1;
            let v = raw
                .get(i)
                .and_then(|x| x.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v > 0.0 && *v <= 1e9);
            a.timeout = Some(v.ok_or_else(|| "--timeout needs a positive number of seconds".to_string())?);
        } else if s == "--backend" {
            let b = operand!("--backend");
            if b != "vm" && b != "llvm" {
                return Err(format!(
                    "unknown backend `{b}` (use --backend vm or --backend llvm)"
                ));
            }
            a.backend = b;
        } else if s.starts_with('-') && s.len() > 1 {
            return Err(format!("unknown option {s}\n{}", usage()));
        } else if a.file.is_none() {
            a.file = Some(s.to_string());
        } else {
            return Err(format!(
                "unexpected extra argument `{s}` (one file operand; use --include for more files)"
            ));
        }
        i += 1;
    }
    if a.cmd != "help" {
        if let Some(f) = &a.file {
            if matches!(
                a.cmd.as_str(),
                "version" | "--version" | "-V" | "repl" | "benchmark" | "fuzz"
            ) {
                return Err(format!("`{}` takes no file operand (found `{f}`)", a.cmd));
            }
        }
    }
    a.color = color_mode.unwrap_or_else(auto_color);
    Ok(a)
}

/// Colour diagnostics only when stderr is a terminal and `NO_COLOR` is unset
/// or empty (<https://no-color.org>); `--color` / `--no-color` override.
fn auto_color() -> bool {
    use std::io::IsTerminal;
    let no_color = env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    !no_color && io::stderr().is_terminal()
}

fn main() -> ExitCode {
    // Every command runs on a thread with a large stack so that deeply
    // nested programs never overflow the main thread.
    let worker = std::thread::Builder::new()
        .name("aether".into())
        .stack_size(WORKER_STACK_BYTES)
        .spawn(real_main);
    match worker {
        Ok(handle) => match handle.join() {
            Ok(code) => code,
            Err(_) => {
                eprintln!("aether: internal error (worker thread panicked)");
                ExitCode::from(101)
            }
        },
        Err(e) => {
            eprintln!("aether: cannot start worker thread ({e}); running on the main thread");
            real_main()
        }
    }
}

fn real_main() -> ExitCode {
    match parse_args() {
        Ok(args) => match dispatch(args) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(1)
            }
        },
        Err(e) => {
            eprint!("{e}");
            if !e.ends_with('\n') {
                eprintln!();
            }
            ExitCode::from(1)
        }
    }
}

fn copts(opt_level: u8, color: bool) -> CompileOptions {
    CompileOptions { opt_level, color }
}

/// `AETHER_INCLUDE` entries first, then every `--include` in command-line order.
fn includes(a: &Args) -> Vec<String> {
    let mut v = default_includes();
    v.extend(a.includes.iter().cloned());
    v
}

/// Compile the file operand together with the include list.
fn compile_input(a: &Args, opt_level: u8, color: bool) -> Result<Compiled, String> {
    compile_files(need_file(a)?, &includes(a), &copts(opt_level, color))
}

fn vm_opts(a: &Args) -> VmOptions {
    let d = VmOptions::default();
    VmOptions {
        max_steps: a.max_steps.unwrap_or(d.max_steps),
        max_call_depth: a.max_depth.unwrap_or(d.max_call_depth),
        ..d
    }
}

/// Emit diagnostics and return the compile-error exit code when there are errors.
fn check_errors(c: &Compiled, color: bool) -> Option<ExitCode> {
    if c.diags.has_errors() {
        c.diags.emit(&c.session, color);
        Some(ExitCode::from(1))
    } else {
        None
    }
}

fn dispatch(a: Args) -> Result<ExitCode, String> {
    match a.cmd.as_str() {
        "help" | "--help" | "-h" => {
            outp!("{}", usage());
            Ok(ExitCode::SUCCESS)
        }
        "version" | "--version" | "-V" => {
            outln!("aether {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        "check" => {
            let c = compile_input(&a, 0, a.color)?;
            c.diags.emit(&c.session, a.color);
            if c.diags.has_errors() {
                Ok(ExitCode::from(1))
            } else {
                outln!("ok");
                Ok(ExitCode::SUCCESS)
            }
        }
        "run" => {
            let mut c = compile_input(&a, a.opt, a.color)?;
            if let Some(code) = check_errors(&c, a.color) {
                return Ok(code);
            }
            if a.backend == "llvm" {
                return run_llvm(&c, &a);
            }
            match run_streaming(&mut c, &a) {
                Ok((val, steps)) => {
                    if a.stats {
                        eprintln!("exit = {val}");
                        eprintln!("vm steps = {steps}");
                        if let Some(r) = &c.opt_report {
                            eprint!("{}", r.summary());
                        }
                    }
                    if a.timings {
                        eprint!("{}", c.timings.render());
                    }
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) => {
                    eprintln!("runtime error: {e}");
                    Ok(ExitCode::from(2))
                }
            }
        }
        "compile" => {
            let c = compile_input(&a, a.opt, a.color)?;
            if let Some(code) = check_errors(&c, a.color) {
                return Ok(code);
            }
            let text = match a.emit.as_str() {
                "ir" => dump_ir_text(&c, false),
                "llvm" => c.llvm.clone().unwrap_or_default(),
                _ => dump_bc(&c),
            };
            if let Some(out) = a.output {
                std::fs::write(&out, text).map_err(|e| format!("cannot write {out}: {e}"))?;
            } else {
                outp!("{text}");
            }
            Ok(ExitCode::SUCCESS)
        }
        "dump-tokens" => {
            let c = compile_input(&a, 0, false)?;
            outp!("{}", dump_tokens(&c));
            // the tokens are dumped even when the lexer rejected part of the
            // input, but its errors (E00xx) are reported and fail the command
            let lexical = |d: &aether::diagnostic::Diagnostic| {
                d.level == aether::diagnostic::Level::Error
                    && d.code.map_or(false, |k| k.starts_with("E00"))
            };
            if c.diags.iter().any(lexical) {
                let mut lex = aether::diagnostic::Diagnostics::new();
                for d in c.diags.iter().filter(|d| lexical(d)) {
                    lex.push(d.clone());
                }
                lex.emit(&c.session, a.color);
                return Ok(ExitCode::from(1));
            }
            Ok(ExitCode::SUCCESS)
        }
        "dump-ast" => {
            let c = compile_input(&a, 0, false)?;
            if let Some(code) = check_errors(&c, a.color) {
                return Ok(code);
            }
            outp!("{}", dump_ast(&c));
            Ok(ExitCode::SUCCESS)
        }
        "dump-ir" => {
            let c = compile_input(&a, a.opt, a.color)?;
            if let Some(code) = check_errors(&c, a.color) {
                return Ok(code);
            }
            outp!("{}", dump_ir_text(&c, a.unopt));
            Ok(ExitCode::SUCCESS)
        }
        "dump-bytecode" | "disassemble" => {
            let c = compile_input(&a, a.opt, a.color)?;
            if let Some(code) = check_errors(&c, a.color) {
                return Ok(code);
            }
            outp!("{}", dump_bc(&c));
            Ok(ExitCode::SUCCESS)
        }
        "optimize" => {
            let c = compile_input(&a, a.opt, a.color)?;
            if let Some(code) = check_errors(&c, a.color) {
                return Ok(code);
            }
            if let Some(r) = &c.opt_report {
                outp!("{}", r.summary());
            }
            Ok(ExitCode::SUCCESS)
        }
        "dump-llvm" => {
            let c = compile_input(&a, a.opt, a.color)?;
            if let Some(code) = check_errors(&c, a.color) {
                return Ok(code);
            }
            outp!("{}", c.llvm.clone().unwrap_or_default());
            Ok(ExitCode::SUCCESS)
        }
        "fmt" => {
            // Formats the main file only: the items of imported and included
            // files are not part of its text (`use` lines are kept as written).
            let mut c = compile_file(need_file(&a)?, &copts(0, false))?;
            // lexical/syntax errors (E00xx/E01xx) stop formatting; type errors and
            // unresolved imports do not, since only the syntax tree is printed
            let syntax = |d: &aether::diagnostic::Diagnostic| {
                d.level == aether::diagnostic::Level::Error
                    && d.code.map_or(true, |k| k.starts_with("E00") || k.starts_with("E01"))
            };
            if c.diags.iter().any(syntax) {
                c.diags.emit(&c.session, a.color);
                return Ok(ExitCode::from(1));
            }
            let main_file = c.file;
            c.program.items.retain(|it| it.span().file == main_file);
            let src = c.session.file(main_file).map_or("", |f| f.source.as_str());
            outp!("{}", aether::comments::format_program(&c.program, main_file, src));
            Ok(ExitCode::SUCCESS)
        }
        "cfg" => {
            let c = compile_input(&a, a.opt, false)?;
            if let Some(code) = check_errors(&c, a.color) {
                return Ok(code);
            }
            let ir = c.ir.as_ref().ok_or_else(|| "no IR".to_string())?;
            outp!("{}", aether::ir::cfg::to_dot(ir));
            Ok(ExitCode::SUCCESS)
        }
        "dump-hir" => {
            let c = compile_input(&a, 0, false)?;
            if let Some(code) = check_errors(&c, a.color) {
                return Ok(code);
            }
            match &c.hir {
                Some(h) => outp!("{h:#?}"),
                None => eprintln!("no HIR"),
            }
            Ok(ExitCode::SUCCESS)
        }
        "dump-liveness" => {
            let c = compile_input(&a, a.opt, false)?;
            if let Some(code) = check_errors(&c, a.color) {
                return Ok(code);
            }
            let ir = c.ir.as_ref().ok_or_else(|| "no IR".to_string())?;
            outp!("{}", aether::opt::liveness::render(ir));
            Ok(ExitCode::SUCCESS)
        }
        "verify" => {
            let c = compile_input(&a, a.opt, false)?;
            if let Some(code) = check_errors(&c, a.color) {
                return Ok(code);
            }
            let ir = c.ir.as_ref().ok_or_else(|| "no IR".to_string())?;
            match aether::ir::verify::verify_module(ir) {
                Ok(()) => {
                    outln!("verify: ok ({} functions)", ir.functions.len());
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) => {
                    eprintln!("verify: {e}");
                    Ok(ExitCode::from(1))
                }
            }
        }
        "stats" => {
            let c = compile_input(&a, a.opt, a.color)?;
            if let Some(code) = check_errors(&c, a.color) {
                return Ok(code);
            }
            if let Some(r) = &c.opt_report {
                outp!("{}", r.summary());
            }
            if let Some(ir) = &c.ir {
                outln!(
                    "functions={} blocks={} insts={}",
                    ir.functions.len(),
                    ir.functions.iter().map(|f| f.blocks.len()).sum::<usize>(),
                    ir_inst_count(&c)
                );
                outp!("{}", aether::opt::liveness::render(ir));
            }
            Ok(ExitCode::SUCCESS)
        }
        "profile" => {
            let c = compile_input(&a, a.opt, a.color)?;
            if let Some(code) = check_errors(&c, a.color) {
                return Ok(code);
            }
            let bc = c.bytecode.as_ref().ok_or_else(|| "no bytecode".to_string())?;
            // a single profiled run, with the requested limits; on failure
            // it still hands back the partial stdout
            let (result, out, report) = aether::vm::execute_profiled_with(bc, vm_opts(&a));
            outp!("{out}");
            match result {
                Ok(val) => {
                    outln!("=> {val}");
                    outp!("{}", report.render());
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) => {
                    eprintln!("runtime error: {e}");
                    Ok(ExitCode::from(2))
                }
            }
        }
        "digest" => {
            let mut c = compile_input(&a, a.opt, false)?;
            if let Some(code) = check_errors(&c, a.color) {
                return Ok(code);
            }
            match run_compiled_with(&mut c, vm_opts(&a)) {
                Ok((val, out, _)) => {
                    outln!("{:#x}", aether::vm::digest(&val, &out));
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) => {
                    outp!("{}", e.stdout);
                    eprintln!("runtime error: {e}");
                    Ok(ExitCode::from(2))
                }
            }
        }
        "bench" => bench(&a),
        "repl" => repl(&a),
        "benchmark" => benchmark(a.n.unwrap_or(20), &a),
        "fuzz" => run_fuzz_cmd(a.iters, a.seed, &a.kind),
        other => Err(format!("unknown command `{other}`\n{}", usage())),
    }
}

fn need_file(a: &Args) -> Result<&str, String> {
    a.file.as_deref().ok_or_else(|| "missing file operand".into())
}

/// Default wall-clock limit for `run --backend llvm` (seconds): `lli` has no
/// instruction budget, so this is what stops a runaway program there.
const LLVM_DEFAULT_TIMEOUT_SECS: f64 = 10.0;

/// Shared handle to a buffered stdout: the VM writes through one clone and
/// the CLI flushes through the other once the run is over.
#[derive(Clone)]
struct StdoutSink(std::rc::Rc<std::cell::RefCell<Box<dyn Write>>>);

impl Write for StdoutSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.borrow_mut().flush()
    }
}

/// `run` on the VM, streaming the program's output to stdout as it is
/// produced (nothing is held back until the end, and a closed pipe stops the
/// program at its next write). `--timeout` is checked between slices of
/// 2^20 instructions.
fn run_streaming(c: &mut Compiled, a: &Args) -> Result<(aether::vm::Value, u64), String> {
    use aether::vm::{Step, Vm, VmError};
    let bc = c.bytecode.as_ref().ok_or_else(|| VmError::MissingMain.to_string())?;
    // line-buffered on a terminal (output shows up as it is printed),
    // block-buffered into a pipe or file (one syscall per 8 KiB, not per line)
    let w: Box<dyn Write> = if io::IsTerminal::is_terminal(&io::stdout()) {
        Box::new(io::stdout())
    } else {
        Box::new(io::BufWriter::new(io::stdout()))
    };
    let sink = StdoutSink(std::rc::Rc::new(std::cell::RefCell::new(w)));
    let deadline = a
        .timeout
        .map(|t| std::time::Instant::now() + std::time::Duration::from_secs_f64(t));
    let t0 = std::time::Instant::now();
    let mut vm = Vm::new(bc, vm_opts(a)).with_stdout(Box::new(sink.clone()));
    let result = loop {
        match vm.run_budget(1 << 20) {
            Ok(Step::Finished(v)) => break Ok(v),
            Ok(Step::Yielded) => {
                if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                    break Err(VmError::Runtime("time limit exceeded".into()));
                }
            }
            Err(e) => break Err(e),
        }
    };
    let exec_us = t0.elapsed().as_micros();
    let steps = vm.steps();
    drop(vm);
    c.timings.exec_us = exec_us;
    let flushed = sink.0.borrow_mut().flush();
    match result {
        Err(VmError::OutputClosed) => std::process::exit(0),
        _ => {
            if let Err(e) = flushed {
                stdout_failed(&e);
            }
        }
    }
    result.map(|v| (v, steps)).map_err(|e| e.to_string())
}

/// `run --backend llvm`: hand the textual LLVM IR to `lli` and relay its stdout.
/// Under `lli` the value `main` returns is the process exit status (mod 256),
/// so it is the program's result, not a failure: like the VM backend, the
/// command exits 0 (`--stats` prints it). A signal is a runtime error
/// (SIGABRT: failed `assert`, division by zero, bad index; exit 2); a
/// non-zero status together with `lli` diagnostics means `lli` rejected the
/// module (exit 1). `lli` is killed after `--timeout` seconds (default 10):
/// "runtime error: time limit exceeded", exit 2.
fn run_llvm(c: &Compiled, a: &Args) -> Result<ExitCode, String> {
    let ir = c
        .llvm
        .as_deref()
        .ok_or_else(|| "no LLVM IR was produced for this program".to_string())?;
    let secs = a.timeout.unwrap_or(LLVM_DEFAULT_TIMEOUT_SECS);
    let run = match aether::driver::run_llvm_ir_with(ir, Some(std::time::Duration::from_secs_f64(secs))) {
        Ok(r) => r,
        Err(e) if e.contains("not found") => {
            eprintln!(
                "error: --backend llvm needs LLVM's `lli` (or `lli-18`) on PATH: {e}\n\
                 hint: install LLVM, or run on the bytecode VM with --backend vm"
            );
            return Ok(ExitCode::from(1));
        }
        Err(e) => {
            eprintln!("llvm backend error: {e}");
            return Ok(ExitCode::from(1));
        }
    };
    outp!("{}", run.stdout);
    match run.status {
        LliStatus::Exited(code) if code == 0 || run.stderr.trim().is_empty() => {
            if a.stats {
                eprintln!("exit = {code} (main's value, mod 256, as reported by lli)");
            }
            if a.timings {
                eprint!("{}", c.timings.render());
            }
            Ok(ExitCode::SUCCESS)
        }
        LliStatus::Exited(_) => {
            eprintln!("llvm backend error: {}", run.stderr.trim_end());
            Ok(ExitCode::from(1))
        }
        LliStatus::Signaled(6) => {
            // the runtime prints the VM's message before `abort()`; relay it
            // (lli's own crash report after it is noise)
            match run.stderr.lines().find(|l| l.starts_with("runtime error: ")) {
                Some(line) => eprintln!("{line}"),
                None => eprintln!(
                    "runtime error: program aborted (failed assert, division by zero, index out of bounds \
                     or an `extern fn` without an implementation)"
                ),
            }
            Ok(ExitCode::from(2))
        }
        LliStatus::Signaled(11) => {
            // SIGSEGV: the program ran off the native stack (the LLVM backend
            // has no call-depth limit), not a fault of the backend
            eprintln!(
                "runtime error: the program crashed under lli (SIGSEGV; most likely a native \
                 stack overflow from deep recursion, which the VM reports as `call stack overflow`)"
            );
            Ok(ExitCode::from(2))
        }
        LliStatus::Signaled(sig) => {
            eprintln!("llvm backend error: lli was killed by signal {sig}");
            Ok(ExitCode::from(1))
        }
        LliStatus::TimedOut => {
            eprintln!("runtime error: time limit exceeded ({secs} s; raise it with --timeout)");
            Ok(ExitCode::from(2))
        }
    }
}

// ---------------------------------------------------------------------------
// REPL with state
// ---------------------------------------------------------------------------

/// One item (`fn` / `struct` / `extern fn`) kept across REPL inputs.
struct ReplItem {
    name: String,
    src: String,
}

fn repl(a: &Args) -> Result<ExitCode, String> {
    outln!("Aether REPL — blank line to run; :items lists definitions, :reset clears them, :quit exits");
    let stdin = io::stdin();
    let mut items: Vec<ReplItem> = Vec::new();
    loop {
        outp!("aether> ");
        let mut buf = String::new();
        let mut eof = false;
        loop {
            let mut line = String::new();
            if stdin.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                eof = true;
                break;
            }
            let t = line.trim();
            match t {
                ":quit" | ":exit" | ":q" => return Ok(ExitCode::SUCCESS),
                ":reset" => {
                    items.clear();
                    outln!("(definitions cleared)");
                }
                ":items" => {
                    if items.is_empty() {
                        outln!("(no definitions)");
                    } else {
                        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
                        outln!("{}", names.join(", "));
                    }
                }
                ":help" => outln!(
                    "an input ends at a blank line. Inputs starting with fn/struct/enum/extern/pub/use\n\
                     are definitions and are kept (one defining `fn main` runs as a whole program);\n\
                     anything else runs inside a fresh `main` (a final expression without `;` shows\n\
                     its i32 value).\n\
                     :items  list kept definitions\n:reset  forget them\n:help   this text\n:quit   exit (also :exit, :q)"
                ),
                "" => break,
                cmd if cmd.starts_with(':') => {
                    eprintln!("unknown REPL command `{cmd}` (try :help)");
                }
                _ => buf.push_str(&line),
            }
        }
        if !buf.trim().is_empty() {
            repl_eval(&mut items, buf, a);
        }
        if eof {
            outln!();
            return Ok(ExitCode::SUCCESS);
        }
    }
}

/// Kept items as `(file name, source)` pairs, skipping the names in `skip`.
fn repl_files(items: &[ReplItem], skip: &[String]) -> Vec<(String, String)> {
    items
        .iter()
        .filter(|it| !skip.contains(&it.name))
        .map(|it| (format!("<repl:{}>", it.name), it.src.clone()))
        .collect()
}

/// The input starts with an item keyword (`fn`, `struct`, `enum`, `extern`,
/// `pub`, `use`) rather than a statement.
fn starts_with_item(head: &str) -> bool {
    let word: String = head
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    matches!(word.as_str(), "fn" | "struct" | "enum" | "extern" | "pub" | "use")
}

fn repl_eval(items: &mut Vec<ReplItem>, buf: String, a: &Args) {
    let opts = copts(a.opt, a.color);
    let head = buf.trim_start();
    if starts_with_item(head) {
        // Definitions: parse alone to learn their names, then type-check them
        // together with the kept items (replacing same-named ones). An error
        // discards only this input.
        let (toks, lex_diags) = aether::lexer::tokenize(FileId(0), &buf);
        let (program, parse_diags) = aether::parser::parse(toks);
        if lex_diags.has_errors() || parse_diags.has_errors() {
            let mut session = Session::new();
            session.add_file("<repl>".into(), buf);
            lex_diags.emit(&session, a.color);
            parse_diags.emit(&session, a.color);
            return;
        }
        if program
            .items
            .iter()
            .any(|it| matches!(it, Item::Fn(f) if f.name.name == "main"))
        {
            // A whole program: run it against the kept definitions, keep nothing.
            let mut files = repl_files(items, &[]);
            files.push(("<repl>".into(), buf));
            repl_run(files, &opts, a);
            return;
        }
        let new_items: Vec<ReplItem> = program
            .items
            .iter()
            .map(|it| {
                let (name, span) = match it {
                    Item::Fn(f) => (f.name.name.clone(), f.span),
                    Item::Struct(s) => (s.name.name.clone(), s.span),
                    Item::Enum(en) => (en.name.name.clone(), en.span),
                    Item::Extern(e) => (e.name.name.clone(), e.span),
                    Item::Use(u) => (format!("use {}", u.path), u.span),
                };
                let (lo, hi) = (span.start.0 as usize, span.end.0 as usize);
                let src = if lo < hi && hi <= buf.len() && buf.is_char_boundary(lo) && buf.is_char_boundary(hi) {
                    buf[lo..hi].to_string()
                } else {
                    buf.clone()
                };
                ReplItem { name, src }
            })
            .collect();
        if new_items.is_empty() {
            outln!("(no definitions found)");
            return;
        }
        let names: Vec<String> = new_items.iter().map(|i| i.name.clone()).collect();
        let mut files = repl_files(items, &names);
        files.push(("<repl>".into(), buf));
        files.push((
            "<repl:main>".into(),
            "fn main() -> i32 { return 0; }\n".into(),
        ));
        let c = compile_sources_public(files, &opts);
        if c.diags.has_errors() {
            c.diags.emit(&c.session, a.color);
            outln!("(input discarded; previous definitions kept)");
            return;
        }
        items.retain(|it| !names.contains(&it.name));
        items.extend(new_items);
        outln!("defined {}", names.join(", "));
    } else {
        // Statements: wrap into a fresh `main` (same line, so line numbers match).
        let kept = repl_files(items, &[]);
        let src = buf.trim_end();
        if !src.ends_with(';') && !src.ends_with('}') {
            // a final expression without `;`: show its value if it is an
            // i32 (`dbl(3)`, `let x = 2; x * 3`), else just evaluate it for
            // its effects (`print_i32(1)` without `;`)
            let mut cands = vec![format!("fn main() -> i32 {{ return ({src}); }}\n")];
            if let Some(cut) = src.rfind(|c| c == ';' || c == '}') {
                let (head, tail) = src.split_at(cut + 1);
                if !tail.trim().is_empty() {
                    cands.push(format!("fn main() -> i32 {{ {head}\nreturn ({}); }}\n", tail.trim()));
                }
            }
            let stmt = format!("fn main() -> i32 {{ {src};\nreturn 0; }}\n");
            cands.push(stmt.clone());
            for cand in cands {
                let mut files = kept.clone();
                files.push(("<repl>".into(), cand));
                if !compile_sources_public(files.clone(), &opts).diags.has_errors() {
                    repl_run(files, &opts, a);
                    return;
                }
            }
            // none compiles: report the errors of the plain statement form
            // (wrapping it as `{src}\nreturn 0;` would only add a bogus
            // "expected `;`" at the synthetic `return`)
            let mut files = kept;
            files.push(("<repl>".into(), stmt));
            repl_run(files, &opts, a);
            return;
        }
        let mut files = kept;
        files.push((
            "<repl>".into(),
            format!("fn main() -> i32 {{ {buf}\nreturn 0; }}\n"),
        ));
        repl_run(files, &opts, a);
    }
}

fn repl_run(files: Vec<(String, String)>, opts: &CompileOptions, a: &Args) {
    let mut c = compile_sources_public(files, opts);
    if c.diags.has_errors() {
        c.diags.emit(&c.session, a.color);
        return;
    }
    match run_compiled_with(&mut c, vm_opts(a)) {
        Ok((val, out, _)) => {
            outp!("{out}");
            outln!("=> {val}");
        }
        Err(e) => {
            outp!("{}", e.stdout);
            eprintln!("runtime error: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

struct BenchRow {
    level: u8,
    min_us: u128,
    median_us: u128,
    steps: u64,
    insts: usize,
    value: String,
    stdout: String,
}

/// `bench FILE [--n N]`: compile at -O0 and -O2, run each N times and compare.
fn bench(a: &Args) -> Result<ExitCode, String> {
    let file = need_file(a)?;
    let runs = a.n.unwrap_or(5);
    if runs < 1 {
        return Err("bench: --n must be at least 1".to_string());
    }
    let runs = runs as usize;
    let incs = includes(a);
    let mut rows: Vec<BenchRow> = Vec::new();
    for level in [0u8, 2u8] {
        let mut c = compile_files(file, &incs, &copts(level, false))?;
        if let Some(code) = check_errors(&c, a.color) {
            return Ok(code);
        }
        let mut times: Vec<u128> = Vec::with_capacity(runs);
        let mut steps = 0u64;
        let mut value = String::new();
        let mut stdout = String::new();
        for _ in 0..runs {
            match run_compiled_with(&mut c, vm_opts(a)) {
                Ok((v, out, s)) => {
                    times.push(c.timings.exec_us);
                    steps = s;
                    value = v.to_string();
                    stdout = out;
                }
                Err(e) => {
                    outp!("{}", e.stdout);
                    eprintln!("runtime error at -O{level}: {e}");
                    return Ok(ExitCode::from(2));
                }
            }
        }
        times.sort_unstable();
        let median_us = if runs % 2 == 1 {
            times[runs / 2]
        } else {
            (times[runs / 2 - 1] + times[runs / 2]) / 2
        };
        rows.push(BenchRow {
            level,
            min_us: times[0],
            median_us,
            steps,
            insts: ir_inst_count(&c),
            value,
            stdout,
        });
    }
    outln!("bench {file}  runs={runs}");
    for r in &rows {
        outln!(
            "  -O{}: exec_us min={} median={}  vm_steps={}  ir_insts={}  result={}",
            r.level, r.min_us, r.median_us, r.steps, r.insts, r.value
        );
    }
    let (o0, o2) = (&rows[0], &rows[1]);
    let ratio = |x: u128, y: u128| x as f64 / (y.max(1)) as f64;
    outln!(
        "  speedup -O2 vs -O0: {:.2}x exec (median), {:.2}x vm steps, {:.2}x ir insts",
        ratio(o0.median_us, o2.median_us),
        ratio(o0.steps as u128, o2.steps as u128),
        ratio(o0.insts as u128, o2.insts as u128)
    );
    if o0.value != o2.value || o0.stdout != o2.stdout {
        eprintln!("warning: -O0 and -O2 produced different results");
    }
    Ok(ExitCode::SUCCESS)
}

fn benchmark(n: i32, a: &Args) -> Result<ExitCode, String> {
    let src = format!(
        "fn fib(n: i32) -> i32 {{\n    if n < 2 {{ return n; }}\n    return fib(n - 1) + fib(n - 2);\n}}\nfn main() -> i32 {{ return fib({n}); }}\n"
    );
    let mut unopt = compile_source("bench.ae", &src, &copts(0, false));
    let mut optc = compile_source("bench.ae", &src, &copts(2, false));
    // like `run`: --max-steps / --max-depth apply, and exceeding them is a
    // runtime error (exit 2)
    let mut results = Vec::with_capacity(2);
    for (level, c) in [(0, &mut unopt), (2, &mut optc)] {
        match run_compiled_with(c, vm_opts(a)) {
            Ok((v, _, s)) => results.push((v, s)),
            Err(e) => {
                eprintln!("runtime error at -O{level}: {e} (fib({n}); raise --max-steps / --max-depth)");
                return Ok(ExitCode::from(2));
            }
        }
    }
    let (v1, s1) = results.pop().expect("two runs");
    let (v0, s0) = results.pop().expect("two runs");
    outln!("fib({n})");
    outln!(
        "  result -O0 = {v0}   steps = {s0}   exec_us = {}",
        unopt.timings.exec_us
    );
    outln!(
        "  result -O2 = {v1}   steps = {s1}   exec_us = {}",
        optc.timings.exec_us
    );
    if let (Some(a), Some(b)) = (&unopt.opt_report, &optc.opt_report) {
        outln!("  IR insts -O0 = {}", a.insts_before);
        outln!("  IR insts -O2 = {}", b.insts_after);
    }
    Ok(ExitCode::SUCCESS)
}

fn parse_u64(s: &str) -> Option<u64> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        s.parse().ok()
    }
}

fn run_fuzz_cmd(iters: u32, seed: u64, kind: &str) -> Result<ExitCode, String> {
    use aether::fuzz::{run_fuzz, FuzzConfig, FuzzKind};
    let kind = FuzzKind::parse(kind).ok_or_else(|| {
        format!("unknown fuzz kind `{kind}` (use all|lexer|parser|pipeline|gen|diff|agg|lang|mut|struct|aspect|mir|greybox|format)")
    })?;
    outln!("aether fuzz  iters={iters} seed={seed:#x} kind={kind:?}");
    let report = run_fuzz(&FuzzConfig { iters, seed, kind });
    outp!("{}", report.summary());
    for f in &report.failures {
        eprintln!(
            "FAIL [{}] seed={} case={}\n{}\n----- source -----\n{}\n",
            f.property, f.seed, f.case, f.detail, f.source
        );
    }
    if report.ok() {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(1))
    }
}

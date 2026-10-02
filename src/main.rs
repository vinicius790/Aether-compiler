//! Aether command-line interface (no external CLI crate).

use aether::ast::Item;
use aether::driver::{
    compile_file, compile_files, compile_source, compile_sources, default_includes, dump_ast,
    dump_bc, dump_ir_text, dump_tokens, ir_inst_count, run_compiled, run_compiled_with,
    CompileOptions, Compiled,
};
use aether::span::{FileId, Session};
use aether::vm::VmOptions;
use std::env;
use std::io::{self, Write};
use std::process::ExitCode;

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
    run <file> [-O<n>] [--timings] [--stats] [--backend vm|llvm]
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
    repl [-O<n>]                  stateful REPL (:items, :reset, :quit)
    benchmark [--n <int>]         built-in fib(n) -O0 vs -O2
    fuzz [--iters N] [--seed N] [--kind all|lexer|parser|pipeline|gen|diff|mut|struct|aspect|mir|greybox|format|agg]
    help
    version

OPTIONS:
    -O<n>, -O <n>         optimizer level 0..2 (default 2)
    --include <file>      compile <file> together with the main file; repeatable
                          (check/run/compile/dump-*/optimize/verify/cfg/stats/
                          profile/digest/bench). AETHER_INCLUDE=a.ae:b.ae adds
                          default includes.
    --max-steps <n>       VM instruction budget (run/profile/digest/bench; default 50000000)
    --max-depth <n>       VM call-depth limit (run/profile/digest/bench; default 10000)
    --backend vm|llvm     `run` on the bytecode VM (default) or through LLVM `lli`
    --timings             per-stage timings on stderr (run)
    --stats               exit value, VM steps and opt report on stderr (run)
    --unopt               dump the IR before optimization (dump-ir)
    --emit ir|bytecode|llvm, -o <path>
                          artifact kind and output file (compile)
    --n <int>             runs per level (bench, default 5); fib argument (benchmark, default 20)
    --iters N, --seed N, --kind K
                          fuzz configuration (seed accepts 0x...)

EXIT CODES:
    0 ok, 1 compile error (or fuzz failure), 2 runtime error
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
    backend: String,
}

fn parse_args() -> Result<Args, String> {
    let mut raw: Vec<String> = env::args().skip(1).collect();
    if raw.is_empty() {
        return Err(usage().into());
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
        color: true,
        iters: 200,
        seed: 0xA37E400,
        kind: "all".into(),
        includes: Vec::new(),
        max_steps: None,
        max_depth: None,
        backend: "vm".into(),
    };
    let mut i = 0;
    while i < raw.len() {
        let s = raw[i].as_str();
        if s == "-h" || s == "--help" {
            return Err(usage().into());
        } else if s.starts_with("-O") && s.len() > 2 {
            a.opt = s[2..].parse().unwrap_or(2);
        } else if s == "-O" {
            i += 1;
            a.opt = raw.get(i).and_then(|x| x.parse().ok()).unwrap_or(2);
        } else if s == "--timings" {
            a.timings = true;
        } else if s == "--stats" {
            a.stats = true;
        } else if s == "--unopt" {
            a.unopt = true;
        } else if s == "--color" {
            a.color = true;
        } else if s == "--emit" {
            i += 1;
            a.emit = raw.get(i).cloned().unwrap_or_else(|| "bytecode".into());
        } else if s == "-o" || s == "--output" {
            i += 1;
            a.output = raw.get(i).cloned();
        } else if s == "--n" {
            i += 1;
            a.n = raw.get(i).and_then(|x| x.parse().ok());
        } else if s == "--iters" {
            i += 1;
            a.iters = raw.get(i).and_then(|x| x.parse().ok()).unwrap_or(200);
        } else if s == "--seed" {
            i += 1;
            a.seed = raw
                .get(i)
                .and_then(|x| parse_u64(x))
                .unwrap_or(0xA37E400);
        } else if s == "--kind" {
            i += 1;
            a.kind = raw.get(i).cloned().unwrap_or_else(|| "all".into());
        } else if s == "--include" || s == "-I" {
            i += 1;
            match raw.get(i) {
                Some(p) if !p.starts_with('-') => a.includes.push(p.clone()),
                _ => return Err(format!("{s} needs a file operand")),
            }
        } else if let Some(p) = s.strip_prefix("--include=") {
            a.includes.push(p.to_string());
        } else if s == "--max-steps" {
            i += 1;
            let v = raw.get(i).and_then(|x| parse_u64(x));
            a.max_steps = Some(v.ok_or_else(|| "--max-steps needs a positive integer".to_string())?);
        } else if s == "--max-depth" {
            i += 1;
            let v = raw.get(i).and_then(|x| x.parse::<usize>().ok());
            a.max_depth = Some(v.ok_or_else(|| "--max-depth needs a positive integer".to_string())?);
        } else if s == "--backend" {
            i += 1;
            let b = raw.get(i).cloned().unwrap_or_default();
            if b != "vm" && b != "llvm" {
                return Err(format!(
                    "unknown backend `{b}` (use --backend vm or --backend llvm)"
                ));
            }
            a.backend = b;
        } else if s.starts_with('-') {
            return Err(format!("unknown option {s}\n{}", usage()));
        } else if a.file.is_none() {
            a.file = Some(s.to_string());
        }
        i += 1;
    }
    Ok(a)
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
            if e.starts_with("Aether compiler") {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
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
            print!("{}", usage());
            Ok(ExitCode::SUCCESS)
        }
        "version" | "--version" | "-V" => {
            println!("aether {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        "check" => {
            let c = compile_input(&a, 0, a.color)?;
            c.diags.emit(&c.session, a.color);
            if c.diags.has_errors() {
                Ok(ExitCode::from(1))
            } else {
                println!("ok");
                Ok(ExitCode::SUCCESS)
            }
        }
        "run" => {
            let mut c = compile_input(&a, a.opt, true)?;
            if let Some(code) = check_errors(&c, true) {
                return Ok(code);
            }
            if a.backend == "llvm" {
                return run_llvm(&c, &a);
            }
            match run_compiled_with(&mut c, vm_opts(&a)) {
                Ok((val, out, steps)) => {
                    print!("{out}");
                    let _ = io::stdout().flush();
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
                    print!("{}", e.stdout);
                    let _ = io::stdout().flush();
                    eprintln!("runtime error: {e}");
                    Ok(ExitCode::from(2))
                }
            }
        }
        "compile" => {
            let c = compile_input(&a, a.opt, true)?;
            if let Some(code) = check_errors(&c, true) {
                return Ok(code);
            }
            let text = match a.emit.as_str() {
                "ir" => dump_ir_text(&c, false),
                "llvm" => c.llvm.clone().unwrap_or_default(),
                _ => dump_bc(&c),
            };
            if let Some(out) = a.output {
                std::fs::write(&out, text).map_err(|e| e.to_string())?;
            } else {
                print!("{text}");
            }
            Ok(ExitCode::SUCCESS)
        }
        "dump-tokens" => {
            let c = compile_input(&a, 0, false)?;
            print!("{}", dump_tokens(&c));
            Ok(ExitCode::SUCCESS)
        }
        "dump-ast" => {
            let c = compile_input(&a, 0, false)?;
            if let Some(code) = check_errors(&c, true) {
                return Ok(code);
            }
            print!("{}", dump_ast(&c));
            Ok(ExitCode::SUCCESS)
        }
        "dump-ir" => {
            let c = compile_input(&a, a.opt, true)?;
            if let Some(code) = check_errors(&c, true) {
                return Ok(code);
            }
            print!("{}", dump_ir_text(&c, a.unopt));
            Ok(ExitCode::SUCCESS)
        }
        "dump-bytecode" | "disassemble" => {
            let c = compile_input(&a, a.opt, true)?;
            if let Some(code) = check_errors(&c, true) {
                return Ok(code);
            }
            print!("{}", dump_bc(&c));
            Ok(ExitCode::SUCCESS)
        }
        "optimize" => {
            let c = compile_input(&a, a.opt, true)?;
            if let Some(code) = check_errors(&c, true) {
                return Ok(code);
            }
            if let Some(r) = &c.opt_report {
                print!("{}", r.summary());
            }
            Ok(ExitCode::SUCCESS)
        }
        "dump-llvm" => {
            let c = compile_input(&a, a.opt, true)?;
            if let Some(code) = check_errors(&c, true) {
                return Ok(code);
            }
            print!("{}", c.llvm.clone().unwrap_or_default());
            Ok(ExitCode::SUCCESS)
        }
        "fmt" => {
            // Formats the main file only: includes are not part of its text.
            let c = compile_file(need_file(&a)?, &copts(0, false))?;
            print!("{}", aether::pretty::pretty_program(&c.program));
            Ok(ExitCode::SUCCESS)
        }
        "cfg" => {
            let c = compile_input(&a, a.opt, false)?;
            if let Some(code) = check_errors(&c, true) {
                return Ok(code);
            }
            let ir = c.ir.as_ref().ok_or_else(|| "no IR".to_string())?;
            print!("{}", aether::ir::cfg::to_dot(ir));
            Ok(ExitCode::SUCCESS)
        }
        "dump-hir" => {
            let c = compile_input(&a, 0, false)?;
            if let Some(code) = check_errors(&c, true) {
                return Ok(code);
            }
            match &c.hir {
                Some(h) => print!("{h:#?}"),
                None => eprintln!("no HIR"),
            }
            Ok(ExitCode::SUCCESS)
        }
        "dump-liveness" => {
            let c = compile_input(&a, a.opt, false)?;
            if let Some(code) = check_errors(&c, true) {
                return Ok(code);
            }
            let ir = c.ir.as_ref().ok_or_else(|| "no IR".to_string())?;
            print!("{}", aether::opt::liveness::render(ir));
            Ok(ExitCode::SUCCESS)
        }
        "verify" => {
            let c = compile_input(&a, a.opt, false)?;
            if let Some(code) = check_errors(&c, true) {
                return Ok(code);
            }
            let ir = c.ir.as_ref().ok_or_else(|| "no IR".to_string())?;
            match aether::ir::verify::verify_module(ir) {
                Ok(()) => {
                    println!("verify: ok ({} functions)", ir.functions.len());
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) => {
                    eprintln!("verify: {e}");
                    Ok(ExitCode::from(1))
                }
            }
        }
        "stats" => {
            let c = compile_input(&a, a.opt, true)?;
            if let Some(code) = check_errors(&c, true) {
                return Ok(code);
            }
            if let Some(r) = &c.opt_report {
                print!("{}", r.summary());
            }
            if let Some(ir) = &c.ir {
                println!(
                    "functions={} blocks={} insts={}",
                    ir.functions.len(),
                    ir.functions.iter().map(|f| f.blocks.len()).sum::<usize>(),
                    ir_inst_count(&c)
                );
                print!("{}", aether::opt::liveness::render(ir));
            }
            Ok(ExitCode::SUCCESS)
        }
        "profile" => {
            let mut c = compile_input(&a, a.opt, true)?;
            if let Some(code) = check_errors(&c, true) {
                return Ok(code);
            }
            if a.max_steps.is_some() || a.max_depth.is_some() {
                // The profiling VM runs with the default limits; enforce the
                // requested ones with a plain run first so the limit error
                // (and the partial stdout) is reported exactly like `run`.
                if let Err(e) = run_compiled_with(&mut c, vm_opts(&a)) {
                    print!("{}", e.stdout);
                    let _ = io::stdout().flush();
                    eprintln!("runtime error: {e}");
                    return Ok(ExitCode::from(2));
                }
            }
            let bc = c.bytecode.as_ref().ok_or_else(|| "no bytecode".to_string())?;
            match aether::vm::execute_profiled(bc) {
                Ok((val, out, report)) => {
                    print!("{out}");
                    println!("=> {val}");
                    print!("{}", report.render());
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
            if let Some(code) = check_errors(&c, true) {
                return Ok(code);
            }
            match run_compiled_with(&mut c, vm_opts(&a)) {
                Ok((val, out, _)) => {
                    println!("{:#x}", aether::vm::digest(&val, &out));
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) => {
                    print!("{}", e.stdout);
                    let _ = io::stdout().flush();
                    eprintln!("runtime error: {e}");
                    Ok(ExitCode::from(2))
                }
            }
        }
        "bench" => bench(&a),
        "repl" => repl(&a),
        "benchmark" => benchmark(a.n.unwrap_or(20)),
        "fuzz" => run_fuzz_cmd(a.iters, a.seed, &a.kind),
        other => Err(format!("unknown command `{other}`\n{}", usage())),
    }
}

fn need_file(a: &Args) -> Result<&str, String> {
    a.file.as_deref().ok_or_else(|| "missing file operand".into())
}

/// `run --backend llvm`: hand the textual LLVM IR to `lli` and relay its stdout.
fn run_llvm(c: &Compiled, a: &Args) -> Result<ExitCode, String> {
    let ir = c
        .llvm
        .as_deref()
        .ok_or_else(|| "no LLVM IR was produced for this program".to_string())?;
    match aether::backend::llvm::run_with_lli(ir) {
        Ok(out) => {
            print!("{out}");
            let _ = io::stdout().flush();
            if a.timings {
                eprint!("{}", c.timings.render());
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(e) if e.contains("not found") => {
            eprintln!(
                "error: --backend llvm needs LLVM's `lli` (or `lli-18`) on PATH: {e}\n\
                 hint: install LLVM, or run on the bytecode VM with --backend vm"
            );
            Ok(ExitCode::from(1))
        }
        Err(e) if e.trim_end() == "lli failed:" => {
            // run_with_lli reports any non-zero process status this way; under
            // lli the value returned by `main` *is* the process exit status.
            eprintln!(
                "llvm backend error: the program exited with a non-zero status \
                 (under lli, `main`'s return value is the exit code)"
            );
            Ok(ExitCode::from(1))
        }
        Err(e) => {
            eprintln!("llvm backend error: {}", e.trim_end());
            Ok(ExitCode::from(1))
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
    println!("Aether REPL — blank line to run; :items lists definitions, :reset clears them, :quit exits");
    let stdin = io::stdin();
    let mut items: Vec<ReplItem> = Vec::new();
    loop {
        print!("aether> ");
        let _ = io::stdout().flush();
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
                    println!("(definitions cleared)");
                }
                ":items" => {
                    if items.is_empty() {
                        println!("(no definitions)");
                    } else {
                        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
                        println!("{}", names.join(", "));
                    }
                }
                ":help" => println!(
                    "statements run inside a fresh `main`; `fn`/`struct`/`extern fn` inputs are kept.\n\
                     :items  list kept definitions\n:reset  forget them\n:quit   exit"
                ),
                "" => break,
                _ => buf.push_str(&line),
            }
        }
        if !buf.trim().is_empty() {
            repl_eval(&mut items, buf, a);
        }
        if eof {
            println!();
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

fn repl_eval(items: &mut Vec<ReplItem>, buf: String, a: &Args) {
    let opts = copts(a.opt, true);
    let head = buf.trim_start();
    if buf.contains("fn main") {
        // A whole program: run it against the kept definitions, keep nothing.
        let mut files = repl_files(items, &[]);
        files.push(("<repl>".into(), buf));
        repl_run(files, &opts, a);
    } else if head.starts_with("fn ") || head.starts_with("struct ") || head.starts_with("extern ") {
        // Definitions: parse alone to learn their names, then type-check them
        // together with the kept items (replacing same-named ones). An error
        // discards only this input.
        let (toks, lex_diags) = aether::lexer::tokenize(FileId(0), &buf);
        let (program, parse_diags) = aether::parser::parse(toks);
        if lex_diags.has_errors() || parse_diags.has_errors() {
            let mut session = Session::new();
            session.add_file("<repl>".into(), buf);
            lex_diags.emit(&session, true);
            parse_diags.emit(&session, true);
            return;
        }
        let new_items: Vec<ReplItem> = program
            .items
            .iter()
            .map(|it| {
                let (name, span) = match it {
                    Item::Fn(f) => (f.name.name.clone(), f.span),
                    Item::Struct(s) => (s.name.name.clone(), s.span),
                    Item::Extern(e) => (e.name.name.clone(), e.span),
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
            println!("(no definitions found)");
            return;
        }
        let names: Vec<String> = new_items.iter().map(|i| i.name.clone()).collect();
        let mut files = repl_files(items, &names);
        files.push(("<repl>".into(), buf));
        files.push((
            "<repl:main>".into(),
            "fn main() -> i32 { return 0; }\n".into(),
        ));
        let c = compile_sources(files, &opts);
        if c.diags.has_errors() {
            c.diags.emit(&c.session, true);
            println!("(input discarded; previous definitions kept)");
            return;
        }
        items.retain(|it| !names.contains(&it.name));
        items.extend(new_items);
        println!("defined {}", names.join(", "));
    } else {
        // Statements: wrap into a fresh `main` (same line, so line numbers match).
        let mut files = repl_files(items, &[]);
        files.push((
            "<repl>".into(),
            format!("fn main() -> i32 {{ {buf}\nreturn 0; }}\n"),
        ));
        repl_run(files, &opts, a);
    }
}

fn repl_run(files: Vec<(String, String)>, opts: &CompileOptions, a: &Args) {
    let mut c = compile_sources(files, opts);
    if c.diags.has_errors() {
        c.diags.emit(&c.session, true);
        return;
    }
    match run_compiled_with(&mut c, vm_opts(a)) {
        Ok((val, out, _)) => {
            print!("{out}");
            println!("=> {val}");
        }
        Err(e) => {
            print!("{}", e.stdout);
            let _ = io::stdout().flush();
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
    let runs = a.n.unwrap_or(5).max(1) as usize;
    let incs = includes(a);
    let mut rows: Vec<BenchRow> = Vec::new();
    for level in [0u8, 2u8] {
        let mut c = compile_files(file, &incs, &copts(level, false))?;
        if let Some(code) = check_errors(&c, true) {
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
                    print!("{}", e.stdout);
                    let _ = io::stdout().flush();
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
    println!("bench {file}  runs={runs}");
    for r in &rows {
        println!(
            "  -O{}: exec_us min={} median={}  vm_steps={}  ir_insts={}  result={}",
            r.level, r.min_us, r.median_us, r.steps, r.insts, r.value
        );
    }
    let (o0, o2) = (&rows[0], &rows[1]);
    let ratio = |x: u128, y: u128| x as f64 / (y.max(1)) as f64;
    println!(
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

fn benchmark(n: i32) -> Result<ExitCode, String> {
    let src = format!(
        "fn fib(n: i32) -> i32 {{\n    if n < 2 {{ return n; }}\n    return fib(n - 1) + fib(n - 2);\n}}\nfn main() -> i32 {{ return fib({n}); }}\n"
    );
    let mut unopt = compile_source("bench.ae", &src, &copts(0, false));
    let mut optc = compile_source("bench.ae", &src, &copts(2, false));
    let (v0, _, s0) = run_compiled(&mut unopt).map_err(|e| e.to_string())?;
    let (v1, _, s1) = run_compiled(&mut optc).map_err(|e| e.to_string())?;
    println!("fib({n})");
    println!(
        "  result -O0 = {v0}   steps = {s0}   exec_us = {}",
        unopt.timings.exec_us
    );
    println!(
        "  result -O2 = {v1}   steps = {s1}   exec_us = {}",
        optc.timings.exec_us
    );
    if let (Some(a), Some(b)) = (&unopt.opt_report, &optc.opt_report) {
        println!("  IR insts -O0 = {}", a.insts_before);
        println!("  IR insts -O2 = {}", b.insts_after);
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
        format!("unknown fuzz kind `{kind}` (use all|lexer|parser|pipeline|gen|diff|mut|struct|aspect|mir|greybox|format|agg)")
    })?;
    println!("aether fuzz  iters={iters} seed={seed:#x} kind={kind:?}");
    let report = run_fuzz(&FuzzConfig { iters, seed, kind });
    print!("{}", report.summary());
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

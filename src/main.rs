//! Aether command-line interface (no external CLI crate).

use aether::driver::{
    compile_file, compile_source, dump_ast, dump_bc, dump_ir_text, dump_tokens, run_compiled,
    CompileOptions,
};
use std::env;
use std::io::{self, Write};
use std::process::ExitCode;

fn usage() -> &'static str {
    "Aether compiler, optimizer and VM

USAGE:
    aether <command> [options] [file]

COMMANDS:
    check <file>                  type-check only
    run <file> [-O<n>] [--timings] [--stats]
    compile <file> [-O<n>] [--emit ir|bytecode|llvm] [-o <path>]
    dump-tokens <file>
    dump-ast <file>
    dump-ir <file> [-O<n>] [--unopt]
    dump-bytecode <file> [-O<n>]
    disassemble <file> [-O<n>]
    dump-llvm <file> [-O<n>]
    optimize <file> [-O<n>]
    fmt <file>                    pretty-print AST back to source
    cfg <file> [-O<n>]            Graphviz DOT of the IR CFG
    verify <file> [-O<n>]         structural IR verifier
    dump-hir <file>               typed HIR
    dump-liveness <file> [-O<n>]  live-in sets per block
    stats <file> [-O<n>]          opt report + liveness + IR size
    profile <file> [-O<n>]        call counts + execution digest
    digest <file> [-O<n>]         deterministic stdout+value fingerprint
    repl [-O<n>]
    benchmark [--n <int>]
    fuzz [--iters N] [--seed N] [--kind all|lexer|parser|pipeline|gen|diff|mut|struct|aspect|mir|greybox|format]
    help
    version
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
    n: i32,
    color: bool,
    iters: u32,
    seed: u64,
    kind: String,
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
        n: 20,
        color: true,
        iters: 200,
        seed: 0xA37E400,
        kind: "all".into(),
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
            a.n = raw.get(i).and_then(|x| x.parse().ok()).unwrap_or(20);
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
            if e.contains("USAGE") {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
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
            let file = need_file(&a)?;
            let c = compile_file(
                file,
                &CompileOptions {
                    opt_level: 0,
                    color: a.color,
                },
            )?;
            c.diags.emit(&c.session, a.color);
            if c.diags.has_errors() {
                Ok(ExitCode::from(1))
            } else {
                println!("ok");
                Ok(ExitCode::SUCCESS)
            }
        }
        "run" => {
            let file = need_file(&a)?;
            let mut c = compile_file(
                file,
                &CompileOptions {
                    opt_level: a.opt,
                    color: true,
                },
            )?;
            if c.diags.has_errors() {
                c.diags.emit(&c.session, true);
                return Ok(ExitCode::from(1));
            }
            match run_compiled(&mut c) {
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
                    eprintln!("runtime error: {e}");
                    Ok(ExitCode::from(2))
                }
            }
        }
        "compile" => {
            let file = need_file(&a)?;
            let c = compile_file(
                file,
                &CompileOptions {
                    opt_level: a.opt,
                    color: true,
                },
            )?;
            if c.diags.has_errors() {
                c.diags.emit(&c.session, true);
                return Ok(ExitCode::from(1));
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
            let c = compile_file(
                need_file(&a)?,
                &CompileOptions {
                    opt_level: 0,
                    color: false,
                },
            )?;
            print!("{}", dump_tokens(&c));
            Ok(ExitCode::SUCCESS)
        }
        "dump-ast" => {
            let c = compile_file(
                need_file(&a)?,
                &CompileOptions {
                    opt_level: 0,
                    color: false,
                },
            )?;
            if c.diags.has_errors() {
                c.diags.emit(&c.session, true);
                return Ok(ExitCode::from(1));
            }
            print!("{}", dump_ast(&c));
            Ok(ExitCode::SUCCESS)
        }
        "dump-ir" => {
            let c = compile_file(
                need_file(&a)?,
                &CompileOptions {
                    opt_level: a.opt,
                    color: true,
                },
            )?;
            if c.diags.has_errors() {
                c.diags.emit(&c.session, true);
                return Ok(ExitCode::from(1));
            }
            print!("{}", dump_ir_text(&c, a.unopt));
            Ok(ExitCode::SUCCESS)
        }
        "dump-bytecode" | "disassemble" => {
            let c = compile_file(
                need_file(&a)?,
                &CompileOptions {
                    opt_level: a.opt,
                    color: true,
                },
            )?;
            if c.diags.has_errors() {
                c.diags.emit(&c.session, true);
                return Ok(ExitCode::from(1));
            }
            print!("{}", dump_bc(&c));
            Ok(ExitCode::SUCCESS)
        }
        "optimize" => {
            let c = compile_file(
                need_file(&a)?,
                &CompileOptions {
                    opt_level: a.opt,
                    color: true,
                },
            )?;
            if c.diags.has_errors() {
                c.diags.emit(&c.session, true);
                return Ok(ExitCode::from(1));
            }
            if let Some(r) = &c.opt_report {
                print!("{}", r.summary());
            }
            Ok(ExitCode::SUCCESS)
        }
        "dump-llvm" => {
            let c = compile_file(
                need_file(&a)?,
                &CompileOptions {
                    opt_level: a.opt,
                    color: true,
                },
            )?;
            if c.diags.has_errors() {
                c.diags.emit(&c.session, true);
                return Ok(ExitCode::from(1));
            }
            print!("{}", c.llvm.clone().unwrap_or_default());
            Ok(ExitCode::SUCCESS)
        }
        "fmt" => {
            let c = compile_file(
                need_file(&a)?,
                &CompileOptions {
                    opt_level: 0,
                    color: false,
                },
            )?;
            print!("{}", aether::pretty::pretty_program(&c.program));
            Ok(ExitCode::SUCCESS)
        }
        "cfg" => {
            let c = compile_file(
                need_file(&a)?,
                &CompileOptions {
                    opt_level: a.opt,
                    color: false,
                },
            )?;
            if c.diags.has_errors() {
                c.diags.emit(&c.session, true);
                return Ok(ExitCode::from(1));
            }
            let ir = c.ir.as_ref().ok_or_else(|| "no IR".to_string())?;
            print!("{}", aether::ir::cfg::to_dot(ir));
            Ok(ExitCode::SUCCESS)
        }
        "dump-hir" => {
            let c = compile_file(
                need_file(&a)?,
                &CompileOptions {
                    opt_level: 0,
                    color: false,
                },
            )?;
            if c.diags.has_errors() {
                c.diags.emit(&c.session, true);
                return Ok(ExitCode::from(1));
            }
            match &c.hir {
                Some(h) => print!("{h:#?}"),
                None => eprintln!("no HIR"),
            }
            Ok(ExitCode::SUCCESS)
        }
        "dump-liveness" => {
            let c = compile_file(
                need_file(&a)?,
                &CompileOptions {
                    opt_level: a.opt,
                    color: false,
                },
            )?;
            if c.diags.has_errors() {
                c.diags.emit(&c.session, true);
                return Ok(ExitCode::from(1));
            }
            let ir = c.ir.as_ref().ok_or_else(|| "no IR".to_string())?;
            print!("{}", aether::opt::liveness::render(ir));
            Ok(ExitCode::SUCCESS)
        }
        "verify" => {
            let c = compile_file(
                need_file(&a)?,
                &CompileOptions {
                    opt_level: a.opt,
                    color: false,
                },
            )?;
            if c.diags.has_errors() {
                c.diags.emit(&c.session, true);
                return Ok(ExitCode::from(1));
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
            let c = compile_file(
                need_file(&a)?,
                &CompileOptions {
                    opt_level: a.opt,
                    color: true,
                },
            )?;
            if c.diags.has_errors() {
                c.diags.emit(&c.session, true);
                return Ok(ExitCode::from(1));
            }
            if let Some(r) = &c.opt_report {
                print!("{}", r.summary());
            }
            if let Some(ir) = &c.ir {
                println!(
                    "functions={} blocks={} insts={}",
                    ir.functions.len(),
                    ir.functions.iter().map(|f| f.blocks.len()).sum::<usize>(),
                    ir.functions
                        .iter()
                        .map(|f| f.blocks.iter().map(|b| b.insts.len()).sum::<usize>())
                        .sum::<usize>()
                );
                print!("{}", aether::opt::liveness::render(ir));
            }
            Ok(ExitCode::SUCCESS)
        }
        "profile" => {
            let mut c = compile_file(
                need_file(&a)?,
                &CompileOptions {
                    opt_level: a.opt,
                    color: true,
                },
            )?;
            if c.diags.has_errors() {
                c.diags.emit(&c.session, true);
                return Ok(ExitCode::from(1));
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
            let mut c = compile_file(
                need_file(&a)?,
                &CompileOptions {
                    opt_level: a.opt,
                    color: false,
                },
            )?;
            if c.diags.has_errors() {
                c.diags.emit(&c.session, true);
                return Ok(ExitCode::from(1));
            }
            match aether::driver::run_compiled(&mut c) {
                Ok((val, out, _)) => {
                    println!("{:#x}", aether::vm::digest(&val, &out));
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) => {
                    eprintln!("runtime error: {e}");
                    Ok(ExitCode::from(2))
                }
            }
        }
        "repl" => repl(a.opt),
        "benchmark" => benchmark(a.n),
        "fuzz" => run_fuzz_cmd(a.iters, a.seed, &a.kind),
        other => Err(format!("unknown command `{other}`\n{}", usage())),
    }
}

fn need_file(a: &Args) -> Result<&str, String> {
    a.file.as_deref().ok_or_else(|| "missing file operand".into())
}

fn repl(opt: u8) -> Result<ExitCode, String> {
    println!("Aether REPL — blank line to run, :quit to exit");
    let stdin = io::stdin();
    loop {
        print!("aether> ");
        let _ = io::stdout().flush();
        let mut buf = String::new();
        loop {
            let mut line = String::new();
            if stdin.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                println!();
                return Ok(ExitCode::SUCCESS);
            }
            let t = line.trim();
            if t == ":quit" || t == ":exit" {
                return Ok(ExitCode::SUCCESS);
            }
            if t.is_empty() {
                break;
            }
            buf.push_str(&line);
        }
        if buf.trim().is_empty() {
            continue;
        }
        let src = if buf.contains("fn main") {
            buf
        } else if buf.trim_start().starts_with("fn ") {
            format!("{buf}\nfn main() -> i32 {{ return 0; }}\n")
        } else {
            format!("fn main() -> i32 {{\n{buf}\nreturn 0;\n}}\n")
        };
        let mut c = compile_source(
            "<repl>",
            &src,
            &CompileOptions {
                opt_level: opt,
                color: true,
            },
        );
        if c.diags.has_errors() {
            c.diags.emit(&c.session, true);
            continue;
        }
        match run_compiled(&mut c) {
            Ok((val, out, _)) => {
                print!("{out}");
                println!("=> {val}");
            }
            Err(e) => eprintln!("runtime error: {e}"),
        }
    }
}

fn benchmark(n: i32) -> Result<ExitCode, String> {
    let src = format!(
        "fn fib(n: i32) -> i32 {{\n    if n < 2 {{ return n; }}\n    return fib(n - 1) + fib(n - 2);\n}}\nfn main() -> i32 {{ return fib({n}); }}\n"
    );
    let mut unopt = compile_source(
        "bench.ae",
        &src,
        &CompileOptions {
            opt_level: 0,
            color: false,
        },
    );
    let mut optc = compile_source(
        "bench.ae",
        &src,
        &CompileOptions {
            opt_level: 2,
            color: false,
        },
    );
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
        format!("unknown fuzz kind `{kind}` (use all|lexer|parser|pipeline|gen|diff|mut|struct|aspect|mir|greybox)")
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

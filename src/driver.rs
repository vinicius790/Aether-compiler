//! Compiler driver: source → tokens → AST → HIR → IR → opt → backend.

use crate::ast::{dump_program, Program};
use crate::backend::{assemble, emit_llvm_ir, BytecodeModule};
use crate::diagnostic::Diagnostics;
use crate::ir::{dump_ir, emit_ir, IrModule};
use crate::lexer::tokenize;
use crate::opt::{optimize, OptReport};
use crate::parser::parse;
use crate::sema::{analyze, HirProgram};
use crate::span::{FileId, Session};
use crate::token::Token;
use crate::vm::{execute_captured, Value, VmError};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct CompileOptions {
    pub opt_level: u8,
    pub color: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        CompileOptions {
            opt_level: 2,
            color: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StageTimings {
    pub lex_us: u128,
    pub parse_us: u128,
    pub sema_us: u128,
    pub ir_us: u128,
    pub opt_us: u128,
    pub codegen_us: u128,
    pub exec_us: u128,
}

impl StageTimings {
    pub fn render(&self) -> String {
        format!(
            "timings (µs): lex={} parse={} sema={} ir={} opt={} codegen={} exec={}\n",
            self.lex_us,
            self.parse_us,
            self.sema_us,
            self.ir_us,
            self.opt_us,
            self.codegen_us,
            self.exec_us
        )
    }
}

pub struct Compiled {
    pub session: Session,
    pub file: FileId,
    pub tokens: Vec<Token>,
    pub program: Program,
    pub hir: Option<HirProgram>,
    pub ir: Option<IrModule>,
    pub ir_unopt: Option<IrModule>,
    pub bytecode: Option<BytecodeModule>,
    pub llvm: Option<String>,
    pub opt_report: Option<OptReport>,
    pub diags: Diagnostics,
    pub timings: StageTimings,
}

impl Compiled {
    pub fn ok(&self) -> bool {
        !self.diags.has_errors() && self.bytecode.is_some()
    }
}

pub fn compile_source(name: &str, source: &str, opts: &CompileOptions) -> Compiled {
    let mut session = Session::new();
    let file = session.add_file(name.to_string(), source.to_string());
    let mut timings = StageTimings::default();

    let t0 = Instant::now();
    let (tokens, lex_diags) = tokenize(file, source);
    timings.lex_us = t0.elapsed().as_micros();

    let t1 = Instant::now();
    let (program, parse_diags) = parse(tokens.clone());
    timings.parse_us = t1.elapsed().as_micros();

    let mut diags = Diagnostics::new();
    diags.extend(lex_diags);
    diags.extend(parse_diags);

    if diags.has_errors() {
        return Compiled {
            session,
            file,
            tokens,
            program,
            hir: None,
            ir: None,
            ir_unopt: None,
            bytecode: None,
            llvm: None,
            opt_report: None,
            diags,
            timings,
        };
    }

    let t2 = Instant::now();
    let (hir, sema_diags) = analyze(&program);
    timings.sema_us = t2.elapsed().as_micros();
    diags.extend(sema_diags);

    if hir.is_none() {
        return Compiled {
            session,
            file,
            tokens,
            program,
            hir,
            ir: None,
            ir_unopt: None,
            bytecode: None,
            llvm: None,
            opt_report: None,
            diags,
            timings,
        };
    }

    let t3 = Instant::now();
    let ir_unopt = emit_ir(hir.as_ref().unwrap());
    timings.ir_us = t3.elapsed().as_micros();

    let t4 = Instant::now();
    let (ir, report) = optimize(ir_unopt.clone(), opts.opt_level);
    timings.opt_us = t4.elapsed().as_micros();

    let t5 = Instant::now();
    let bytecode = assemble(&ir);
    let llvm = emit_llvm_ir(&ir);
    timings.codegen_us = t5.elapsed().as_micros();

    Compiled {
        session,
        file,
        tokens,
        program,
        hir,
        ir: Some(ir),
        ir_unopt: Some(ir_unopt),
        bytecode: Some(bytecode),
        llvm: Some(llvm),
        opt_report: Some(report),
        diags,
        timings,
    }
}

pub fn compile_file(path: &str, opts: &CompileOptions) -> Result<Compiled, String> {
    let source = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    if source.len() > 8 * 1024 * 1024 {
        return Err(format!(
            "refusing to compile {path}: file is larger than 8 MiB"
        ));
    }
    Ok(compile_source(path, &source, opts))
}

pub fn run_compiled(c: &mut Compiled) -> Result<(Value, String, u64), VmError> {
    let bc = c.bytecode.as_ref().ok_or(VmError::MissingMain)?;
    let t = Instant::now();
    let result = execute_captured(bc);
    c.timings.exec_us = t.elapsed().as_micros();
    result
}

pub fn dump_tokens(c: &Compiled) -> String {
    let mut s = String::new();
    for t in &c.tokens {
        s.push_str(&format!(
            "{:<12} {:<16} {}:{} {:?}\n",
            format!("{:?}", t.kind),
            t.lexeme,
            t.span.line,
            t.span.column,
            t.span
        ));
    }
    s
}

pub fn dump_ast(c: &Compiled) -> String {
    dump_program(&c.program)
}

pub fn dump_ir_text(c: &Compiled, unopt: bool) -> String {
    let ir = if unopt {
        c.ir_unopt.as_ref()
    } else {
        c.ir.as_ref()
    };
    match ir {
        Some(m) => dump_ir(m),
        None => "<no IR>\n".into(),
    }
}

pub fn dump_bc(c: &Compiled) -> String {
    match &c.bytecode {
        Some(bc) => bc.disassemble(),
        None => "<no bytecode>\n".into(),
    }
}

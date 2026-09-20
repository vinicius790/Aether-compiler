//! Aether compiler library.
//!
//! Pipeline:
//! `source → lexer → parser → AST → sema → IR → optimizer → bytecode|LLVM → VM`

pub mod ast;
pub mod backend;
pub mod diagnostic;
pub mod driver;
pub mod fuzz;
pub mod host;
pub mod ir;
pub mod lexer;
pub mod opt;
pub mod parser;
pub mod pretty;
pub mod sema;
pub mod span;
pub mod token;
pub mod ty;
pub mod vm;
pub mod runtime;

pub use driver::{compile_file, compile_source, CompileOptions, Compiled};
pub use vm::{execute, Value, VmError};

/// Compile and execute a source string on the Aether VM.
pub fn run_source(name: &str, source: &str, opt_level: u8) -> Result<(Value, String, DiagnosticsView), String> {
    let opts = CompileOptions {
        opt_level,
        color: false,
    };
    let mut compiled = compile_source(name, source, &opts);
    if compiled.diags.has_errors() {
        return Err(compiled.diags.render(&compiled.session, false));
    }
    match driver::run_compiled(&mut compiled) {
        Ok((val, out, steps)) => Ok((
            val,
            out,
            DiagnosticsView {
                errors: compiled.diags.error_count(),
                warnings: compiled.diags.warning_count(),
                steps,
                timings: compiled.timings.render(),
            },
        )),
        Err(e) => Err(e.to_string()),
    }
}

#[derive(Debug, Clone)]
pub struct DiagnosticsView {
    pub errors: usize,
    pub warnings: usize,
    pub steps: u64,
    pub timings: String,
}

#[cfg(test)]
mod e2e {
    use super::*;

    #[test]
    fn end_to_end_add() {
        let (v, _, _) = run_source("t.ae", "fn main() -> i32 { return 40 + 2; }", 2).unwrap();
        assert_eq!(v, Value::I32(42));
    }

    #[test]
    fn end_to_end_if() {
        let src = r#"
            fn max(a: i32, b: i32) -> i32 {
                if a > b { return a; }
                return b;
            }
            fn main() -> i32 { return max(3, 9); }
        "#;
        let (v, _, _) = run_source("t.ae", src, 2).unwrap();
        assert_eq!(v, Value::I32(9));
    }
}

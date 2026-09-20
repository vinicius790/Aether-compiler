//! Embedding API for hosts (game engines, REPLs, test harnesses).
//!
//! Catalog §5.5 asked for bindings (`vec2`, entity, input). Aether 0.2
//! exposes the compiler+VM as a library instead of a second language:
//! compile a string, run it, read the value and stdout.

use crate::driver::{compile_source, run_compiled, CompileOptions};
use crate::vm::{digest, execute_profiled, ProfileReport, Value};

#[derive(Debug, Clone)]
pub struct HostResult {
    pub value: Value,
    pub stdout: String,
    pub steps: u64,
    pub digest: u64,
}

pub fn eval(source: &str, opt_level: u8) -> Result<HostResult, String> {
    eval_named("<host>", source, opt_level)
}

pub fn eval_named(name: &str, source: &str, opt_level: u8) -> Result<HostResult, String> {
    let opts = CompileOptions {
        opt_level,
        color: false,
    };
    let mut compiled = compile_source(name, source, &opts);
    if compiled.diags.has_errors() {
        return Err(compiled.diags.render(&compiled.session, false));
    }
    match run_compiled(&mut compiled) {
        Ok((value, stdout, steps)) => {
            let d = digest(&value, &stdout);
            Ok(HostResult {
                value,
                stdout,
                steps,
                digest: d,
            })
        }
        Err(e) => Err(e.to_string()),
    }
}

pub fn profile_source(source: &str, opt_level: u8) -> Result<(Value, ProfileReport), String> {
    let opts = CompileOptions {
        opt_level,
        color: false,
    };
    let compiled = compile_source("<host>", source, &opts);
    if compiled.diags.has_errors() {
        return Err(compiled.diags.render(&compiled.session, false));
    }
    let bc = compiled
        .bytecode
        .as_ref()
        .ok_or_else(|| "no bytecode".to_string())?;
    execute_profiled(bc)
        .map(|(v, _, p)| (v, p))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_eval_add() {
        let r = eval("fn main() -> i32 { return 2 + 3; }", 2).unwrap();
        assert_eq!(r.value, Value::I32(5));
        assert_ne!(r.digest, 0);
    }

    #[test]
    fn host_rejects_bad() {
        let e = eval("fn main() -> i32 { return x; }", 0).unwrap_err();
        assert!(!e.is_empty());
    }
}

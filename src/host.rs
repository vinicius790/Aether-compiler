//! Embedding API for hosts (game engines, REPLs, test harnesses).
//!
//! Catalog §5.5 asked for bindings (`vec2`, entity, input). Aether 0.2
//! exposes the compiler+VM as a library instead of a second language:
//! compile a string, run it, read the value and stdout. [`Host`] adds the
//! binding half: a script declares `extern fn set_score(x: i32);` and the
//! embedder supplies the body as a Rust closure.

use crate::driver::{compile_source, run_compiled, CompileOptions};
use crate::vm::{
    digest, execute_captured_with, execute_profiled, HostFn, ProfileReport, Value, VmError,
    VmOptions,
};

#[derive(Debug, Clone)]
pub struct HostResult {
    pub value: Value,
    pub stdout: String,
    pub steps: u64,
    pub digest: u64,
}

/// Builder for a VM run with host-bound `extern fn`s and custom options.
///
/// ```
/// use aether::host::Host;
/// use aether::vm::Value;
///
/// let r = Host::new()
///     .register("rand", |_args| Ok(Value::I32(4)))
///     .eval("dice.ae", "extern fn rand() -> i32; fn main() -> i32 { return rand(); }", 2)
///     .unwrap();
/// assert_eq!(r.value, Value::I32(4));
/// ```
pub struct Host {
    fns: Vec<(String, HostFn)>,
    pub opts: VmOptions,
}

impl Default for Host {
    fn default() -> Self {
        Host::new()
    }
}

impl Host {
    pub fn new() -> Self {
        Host {
            fns: Vec::new(),
            opts: VmOptions::default(),
        }
    }

    /// Bind `extern fn NAME` to `f`. Arguments arrive already type-checked
    /// against the extern signature; the result is dropped for unit externs.
    pub fn register<F>(mut self, name: &str, f: F) -> Self
    where
        F: FnMut(&[Value]) -> Result<Value, VmError> + 'static,
    {
        self.fns.push((name.to_string(), Box::new(f)));
        self
    }

    /// Compile `source` and run `main` with the registered bindings.
    pub fn eval(self, name: &str, source: &str, opt_level: u8) -> Result<HostResult, String> {
        let copts = CompileOptions {
            opt_level,
            color: false,
        };
        let compiled = compile_source(name, source, &copts);
        if compiled.diags.has_errors() {
            return Err(compiled.diags.render(&compiled.session, false));
        }
        let bc = compiled
            .bytecode
            .as_ref()
            .ok_or_else(|| "no bytecode".to_string())?;
        let (result, stdout, steps) = execute_captured_with(bc, self.opts, self.fns);
        match result {
            Ok(value) => {
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

    #[test]
    fn host_builder_matches_free_eval() {
        let src = "fn main() -> i32 { print_i32(7); return 2 + 3; }";
        let a = eval(src, 2).unwrap();
        let b = Host::new().eval("<host>", src, 2).unwrap();
        assert_eq!(a.value, b.value);
        assert_eq!(a.stdout, b.stdout);
        assert_eq!(a.steps, b.steps);
        assert_eq!(a.digest, b.digest);
    }
}

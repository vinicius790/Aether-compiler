//! `yield;` suspends a budgeted run and is a no-op under `run()`.

use aether::vm::{Step, Vm, VmOptions};
use aether::{compile_source, CompileOptions, Value};

const SRC: &str = "fn main() -> i32 { let mut s = 0; for i in 0..5 { s += i; yield; } return s; }";

fn bytecode(level: u8) -> aether::backend::BytecodeModule {
    let opts = CompileOptions {
        opt_level: level,
        color: false,
    };
    let c = compile_source("y.ae", SRC, &opts);
    assert!(!c.diags.has_errors(), "{}", c.diags.render(&c.session, false));
    c.bytecode.expect("bytecode")
}

#[test]
fn yield_suspends_once_per_iteration_at_every_level() {
    for level in [0, 2] {
        let bc = bytecode(level);
        let mut vm = Vm::new(&bc, VmOptions::default());
        let mut yields = 0;
        let value = loop {
            match vm.run_budget(1_000_000).expect("vm") {
                Step::Yielded => yields += 1,
                Step::Finished(v) => break v,
            }
        };
        assert_eq!(value, Value::I32(10), "-O{level}");
        assert_eq!(yields, 5, "-O{level}: one yield per loop iteration");
        assert!(vm.is_finished());
    }
}

#[test]
fn yield_is_a_no_op_under_run() {
    let (v, out, _) = aether::run_source("y.ae", SRC, 2).expect("run");
    assert_eq!(v, Value::I32(10));
    assert_eq!(out, "");
}

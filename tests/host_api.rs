//! Embedding API: `Host::register` bindings for `extern fn`s and the
//! resumable `Vm::run_budget` loop a game would drive once per frame.

use aether::host::Host;
use aether::vm::{Step, Value, Vm, VmError, VmOptions};
use aether::{compile_source, CompileOptions};
use std::cell::RefCell;
use std::rc::Rc;

const SCORE_SCRIPT: &str = r#"
    extern fn set_score(x: i32);
    fn main() -> i32 {
        let mut i = 0;
        while i < 5 {
            set_score(i * 10);
            i = i + 1;
        }
        return i;
    }
"#;

fn collect_scores(opt_level: u8) -> (Value, Vec<i32>) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    let r = Host::new()
        .register("set_score", move |args| {
            sink.borrow_mut().push(args[0].as_i32());
            Ok(Value::Unit)
        })
        .eval("score.ae", SCORE_SCRIPT, opt_level)
        .expect("script runs");
    let calls = log.borrow().clone();
    (r.value, calls)
}

#[test]
fn register_collects_extern_calls_into_rc_refcell() {
    let (value, calls) = collect_scores(0);
    assert_eq!(value, Value::I32(5));
    assert_eq!(calls, vec![0, 10, 20, 30, 40]);
}

#[test]
fn extern_calls_survive_the_optimizer() {
    assert_eq!(collect_scores(0), collect_scores(2));
}

#[test]
fn extern_return_value_feeds_the_script() {
    let mut rolls = vec![7, 3, 5].into_iter();
    let r = Host::new()
        .register("rand", move |_args| Ok(Value::I32(rolls.next().unwrap_or(0))))
        .eval(
            "rand.ae",
            r#"
            extern fn rand() -> i32;
            fn main() -> i32 {
                let a = rand();
                let b = rand();
                print_i32(a + b);
                return rand();
            }
            "#,
            0,
        )
        .unwrap();
    assert_eq!(r.stdout, "10\n");
    assert_eq!(r.value, Value::I32(5));
    assert!(r.steps > 0);
}

#[test]
fn unbound_extern_still_errors() {
    let e = Host::new()
        .eval(
            "x.ae",
            "extern fn foo(x: i32); fn main() -> i32 { foo(1); return 0; }",
            0,
        )
        .unwrap_err();
    assert!(
        e.contains("extern function `foo` has no implementation in the VM"),
        "{e}"
    );
    // Binding a different name does not help.
    let e2 = Host::new()
        .register("bar", |_| Ok(Value::Unit))
        .eval(
            "x.ae",
            "extern fn foo(x: i32); fn main() -> i32 { foo(1); return 0; }",
            0,
        )
        .unwrap_err();
    assert!(e2.contains("extern function `foo`"), "{e2}");
}

#[test]
fn host_fn_error_aborts_the_script() {
    let e = Host::new()
        .register("fail", |_| Err(VmError::Native("boom from host".into())))
        .eval(
            "f.ae",
            "extern fn fail(); fn main() -> i32 { fail(); return 1; }",
            0,
        )
        .unwrap_err();
    assert_eq!(e, "boom from host");
}

#[test]
fn host_opts_are_honoured() {
    let mut h = Host::new();
    h.opts.max_steps = 50;
    let e = h
        .eval(
            "spin.ae",
            "fn main() -> i32 { let mut i = 0; while i < 1000 { i = i + 1; } return i; }",
            0,
        )
        .unwrap_err();
    assert_eq!(e, VmError::StepLimit.to_string());
}

#[test]
fn run_budget_loop_resumes_and_matches_run() {
    let src = r#"
        fn main() -> i32 {
            let mut i = 0;
            let mut s = 0;
            while i < 100000 {
                s = s + i % 7;
                i = i + 1;
            }
            print_i32(s);
            return s;
        }
    "#;
    let c = compile_source(
        "loop.ae",
        src,
        &CompileOptions {
            opt_level: 0,
            color: false,
        },
    );
    assert!(!c.diags.has_errors(), "{}", c.diags.render(&c.session, false));
    let bc = c.bytecode.as_ref().expect("bytecode");

    let mut whole = Vm::new(bc, VmOptions::default()).with_stdout(Box::new(std::io::sink()));
    let expected = whole.run().unwrap();

    let mut vm = Vm::new(bc, VmOptions::default()).with_stdout(Box::new(std::io::sink()));
    let mut resumes = 0u64;
    let value = loop {
        match vm.run_budget(5_000).unwrap() {
            Step::Finished(v) => break v,
            Step::Yielded => {
                resumes += 1;
                assert!(!vm.is_finished());
                assert_eq!(vm.steps(), 5_000 * resumes);
            }
        }
    };
    assert!(resumes > 1, "resumes = {resumes}");
    assert!(vm.is_finished());
    assert_eq!(value, expected);
    assert_eq!(vm.steps(), whole.steps());
}

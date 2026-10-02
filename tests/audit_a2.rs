//! Audit A2: execution backends (VM, host API, run_budget, LLVM).

use aether::host::Host;
use aether::vm::{Step, Value, Vm, VmOptions};
use aether::{compile_source, CompileOptions};
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

#[derive(Clone, Default)]
struct Buf(Rc<RefCell<Vec<u8>>>);
impl Write for Buf {
    fn write(&mut self, d: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(d);
        Ok(d.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn bytecode(src: &str, level: u8) -> aether::backend::bytecode::BytecodeModule {
    let c = compile_source("a2.ae", src, &CompileOptions { opt_level: level, color: false });
    assert!(!c.diags.has_errors(), "{}", c.diags.render(&c.session, false));
    c.bytecode.expect("bytecode")
}

type Log = Rc<RefCell<Vec<String>>>;

/// Host bindings used by the budget corpus; every call is logged.
fn bind<'a>(mut vm: Vm<'a>, log: &Log) -> Vm<'a> {
    let l = log.clone();
    vm = vm.with_host_fn("note", Box::new(move |a: &[Value]| {
        l.borrow_mut().push(format!("note{a:?}"));
        Ok(Value::Unit)
    }));
    let l = log.clone();
    let mut n = 0i32;
    vm = vm.with_host_fn("tick", Box::new(move |_a: &[Value]| {
        n += 1;
        l.borrow_mut().push(format!("tick{n}"));
        Ok(Value::I32(n))
    }));
    let l = log.clone();
    vm = vm.with_host_fn("echo_s", Box::new(move |a: &[Value]| {
        l.borrow_mut().push(format!("echo{a:?}"));
        Ok(a[0].clone())
    }));
    vm
}

/// (result, stdout, steps, host log) of a whole run or of a run_budget(k) loop.
fn drive(bc: &aether::backend::bytecode::BytecodeModule, budget: Option<u64>) -> (Result<Value, String>, String, u64, Vec<String>) {
    let out = Buf::default();
    let log: Log = Rc::default();
    let opts = VmOptions { max_steps: 2_000_000, max_call_depth: 200, ..VmOptions::default() };
    let mut vm = bind(Vm::new(bc, opts).with_stdout(Box::new(out.clone())), &log);
    let res = match budget {
        None => vm.run().map_err(|e| e.to_string()),
        Some(k) => {
            let mut last = vm.steps();
            loop {
                match vm.run_budget(k) {
                    Ok(Step::Finished(v)) => break Ok(v),
                    Ok(Step::Yielded) => {
                        assert!(vm.steps() - last <= k, "budget {k} overrun");
                        last = vm.steps();
                    }
                    Err(e) => {
                        assert!(vm.run_budget(k).is_err(), "halted VM must stay halted");
                        break Err(e.to_string());
                    }
                }
            }
        }
    };
    let steps = vm.steps();
    let s = String::from_utf8(out.0.borrow().clone()).unwrap();
    let l = log.borrow().clone();
    (res, s, steps, l)
}

const BUDGET_CORPUS: &[&str] = &[
    "fn main() -> i32 { return 7; }",
    "fn main() { print_i32(1); }",
    "fn main() -> i32 { yield; yield; print_i32(2); yield; return 3; }",
    "extern fn tick() -> i32;\nextern fn note(x: i32);\nfn main() -> i32 { let mut s = 0; while s < 20 { s = s + tick(); note(s); yield; } return s; }",
    "extern fn echo_s(s: string) -> string;\nfn main() -> i32 { let mut t = \"é\"; for i in 0..5 { t = echo_s(t + to_string(i)); yield; } println(t); return len(t); }",
    "fn f(n: i32) -> i32 { if n == 0 { let z = 0; return 1 / z; } print_i32(n); return f(n - 1); }\nfn main() -> i32 { return f(4); }",
    "fn r(n: i32) -> i32 { yield; return r(n + 1) + 1; }\nfn main() -> i32 { println(\"go\"); return r(0); }",
    "fn main() -> i32 { let mut i = 0; while true { i = i + 1; if i % 1000 == 0 { yield; } } return i; }",
    "struct P { s: string, a: [i32; 3] }\nextern fn note(x: i32);\nfn g(p: P) -> P { let mut q = p; q.a[1] = q.a[1] + 1; q.s += \"!\"; yield; return q; }\nfn main() -> i32 { let mut p = P { s: \"x\", a: [1, 2, 3] }; for i in 0..4 { p = g(p); note(p.a[1]); } println(p.s); return p.a[1]; }",
    "enum E { A(string), B(i32) }\nfn h(e: E) -> i32 { return match e { E::A(s) => len(s), E::B(n) => n }; }\nfn main() -> i32 { let xs = [E::A(\"日本語\"), E::B(4)]; let mut t = 0; for i in 0..2 { t += h(xs[i]); yield; } assert(t == 8); print_i32(t); assert(t == 9); return t; }",
    "extern fn missing() -> i32;\nfn main() -> i32 { print(\"a\"); yield; return missing(); }",
    "fn main() -> i32 { let a = [1, 2]; let mut i = 0; while true { print_i32(a[i]); i += 1; yield; } return 0; }",
    // a leaf with `yield` is inlined at -O2 together with the `yield`
    "fn y(n: i32) -> i32 { yield; return n + 1; }\nfn main() -> i32 { let mut a = 0; let b = y(a); a = y(b); print_i32(a); return y(a) + y(b); }",
];

#[test]
fn run_budget_matches_run_for_every_small_budget() {
    for src in BUDGET_CORPUS {
        for level in [0u8, 2] {
            let bc = bytecode(src, level);
            let whole = drive(&bc, None);
            for k in [1u64, 2, 3, 5, 1000] {
                let part = drive(&bc, Some(k));
                assert_eq!(whole, part, "-O{level} budget {k}\n{src}");
            }
        }
    }
}

/// A host closure may hand back an enum whose unused payload slots hold
/// junk; equality and `match` look only at the active variant.
#[test]
fn host_enum_with_junk_in_unused_slots_behaves_like_the_variant() {
    let src = "enum E { A, B(i32) }\nextern fn mk() -> E;\nfn main() -> i32 { let e = mk(); print_bool(e == E::A); let xs = [e, E::A]; print_bool(xs[0] == xs[1]); return match e { E::A => 1, E::B(n) => n }; }";
    for level in [0u8, 2] {
        let r = Host::new()
            .register("mk", |_a: &[Value]| Ok(Value::Object(Rc::new(vec![Value::I32(0), Value::I32(5)]))))
            .eval("p.ae", src, level)
            .unwrap();
        assert_eq!((r.value, r.stdout.as_str()), (Value::I32(1), "true\ntrue\n"), "-O{level}");
    }
}

/// (stdout, stderr, exit code) of `aether run FILE -O<level> [--backend B]`.
fn cli(src: &str, level: u8, backend: &str) -> (String, String, i32) {
    let dir = std::env::temp_dir().join(format!("aether_a2_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = 0u64;
    for b in src.bytes() {
        h = h.wrapping_mul(31).wrapping_add(u64::from(b));
    }
    let path = dir.join(format!("p{h:x}.ae"));
    std::fs::write(&path, src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_aether"))
        .args(["run", path.to_str().unwrap(), &format!("-O{level}"), "--backend", backend])
        .output()
        .expect("spawn aether");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn have_lli() -> bool {
    ["lli", "lli-18"].iter().any(|t| {
        std::process::Command::new(t)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

/// Programs whose runtime error must be reported identically (stdout, the
/// `runtime error: ...` line, exit 2) by the VM and, when `lli` exists, by
/// `--backend llvm`. The LLVM module enforces the VM's call-depth limit
/// (it used to recurse until the native stack blew up) and runs on a big
/// stack so deep recursion with large aggregates that the VM runs works.
const SAME_ERROR_CORPUS: &[(&str, &str, &str)] = &[
    (
        "fn r(n: i32) -> i32 { if n == 0 { return 0; } return 1 + r(n - 1); }\nfn main() -> i32 { print_i32(r(9998)); print_i32(r(9999)); return 0; }",
        "9998\n",
        "runtime error: call stack overflow",
    ),
    (
        "fn r(n: i32) -> i32 { return r(n + 1) + 1; }\nfn main() -> i32 { println(\"x\"); return r(0); }",
        "x\n",
        "runtime error: call stack overflow",
    ),
    (
        "struct Big { a: [i64; 64], s: string }\nfn r(b: Big, n: i32) -> i64 { if n == 0 { return b.a[63]; } let mut c = b; c.a[63] += 1; return r(c, n - 1); }\nfn main() -> i32 { let b = Big { a: [0; 64], s: \"x\" }; print_i64(r(b, 9990)); let z = 0; return 1 / z; }",
        "9990\n",
        "runtime error: division by zero",
    ),
    (
        "fn main() -> i32 { let s = \"a😀c\"; print_char(s[1]); let i = 3; print_char(s[i]); return 0; }",
        "😀\n",
        "runtime error: string index out of bounds",
    ),
    (
        "fn main() -> i32 { let a = [1, 2, 3]; let i = 2147483647; print_i32(a[2]); print_i32(a[i]); return 0; }",
        "3\n",
        "runtime error: array index 2147483647 out of bounds",
    ),
    (
        "fn main() -> i32 { print(\"p\"); assert(1 == 2); return 0; }",
        "p",
        "runtime error: assertion failed",
    ),
];

#[test]
fn runtime_errors_match_between_vm_and_llvm() {
    let lli = have_lli();
    for (src, stdout, err) in SAME_ERROR_CORPUS {
        for level in [0u8, 2] {
            let mut backends = vec!["vm"];
            if lli {
                backends.push("llvm");
            }
            for b in backends {
                let (o, e, code) = cli(src, level, b);
                assert_eq!((o.as_str(), code), (*stdout, 2), "-O{level} --backend {b}\n{src}\n{e}");
                assert_eq!(e.lines().last(), Some(*err), "-O{level} --backend {b}\n{src}\n{e}");
            }
        }
    }
}

/// `Postfix ::= Primary (Call | Index | Field)*`: literals are primaries, so
/// `"ab"[1]` indexes the literal (the parser used to stop after it).
#[test]
fn postfix_applies_to_literals() {
    let src = "fn main() -> i32 { print_char(\"a😀c\"[1]); print_bool(\"日本\"[1] == '本'); let n = len(\"xyz\"); return n + [4, 5][1] + (6, 7).1; }";
    for level in [0u8, 2] {
        let (o, e, code) = cli(src, level, "vm");
        assert_eq!((o.as_str(), code), ("😀\ntrue\n", 0), "-O{level}\n{e}");
        let r = aether::host::eval(src, level).unwrap();
        assert_eq!(r.value, Value::I32(15), "-O{level}");
    }
    for (src, msg) in [
        ("fn main() -> i32 { return \"ab\".len; }", "no field `len` on type `string`"),
        ("fn main() -> i32 { return 1(2); }", "E0256"),
    ] {
        let (_, e, code) = cli(src, 0, "vm");
        assert_eq!(code, 1, "{src}\n{e}");
        assert!(e.contains(msg), "{src}\n{e}");
    }
}

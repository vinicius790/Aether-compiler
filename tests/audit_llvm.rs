//! Differential test: the LLVM backend (run with `lli`) must print exactly
//! what the VM prints and fail (or not) the same way. Skipped when LLVM's
//! `lli` is not on PATH, so CI without LLVM still passes.

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_aether"));
    c.current_dir(env!("CARGO_MANIFEST_DIR"));
    c
}

fn have_lli() -> bool {
    ["lli", "lli-18"].iter().any(|t| {
        Command::new(t)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

/// (stdout, failed?) for one backend.
fn run(path: &Path, backend: &str) -> (String, bool) {
    let out = bin()
        .args(["run", path.to_str().unwrap(), "-O2", "--backend", backend])
        .output()
        .expect("spawn aether");
    let code = out.status.code().unwrap_or(-1);
    (String::from_utf8_lossy(&out.stdout).into_owned(), code != 0)
}

fn check(path: &Path, failures: &mut Vec<String>) {
    let vm = run(path, "vm");
    let ll = run(path, "llvm");
    if vm != ll {
        failures.push(format!(
            "{}\n  vm  : failed={} stdout={:?}\n  llvm: failed={} stdout={:?}",
            path.display(),
            vm.1,
            truncate(&vm.0),
            ll.1,
            truncate(&ll.0)
        ));
    }
}

fn truncate(s: &str) -> String {
    if s.len() > 400 {
        format!("{}…", &s[..400])
    } else {
        s.to_string()
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("aether_llvm_{}_{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

const HAND: &[(&str, &str)] = &[
    ("copy", "fn main() -> i32 { let a = [1, 2, 3]; let mut b = a; b[0] = 9; print_i32(a[0]); print_i32(b[0]); return 0; }"),
    ("callee_copy", "struct P { x: i32 }\nfn f(p: P) -> i32 { let mut q = p; q.x = 7; return q.x; }\nfn main() -> i32 { let p = P { x: 1 }; print_i32(f(p)); print_i32(p.x); return 0; }"),
    ("strings", "fn main() -> i32 { let s = \"ação\" + \"!\"; println(s); print_i32(len(s)); print_char(s[1]); print_bool(s == \"ação!\"); print_bool(\"ab\" < \"b\"); return 0; }"),
    ("enum_mixed", "enum E { A(i32, f64), B(string, i64), C }\nfn show(e: E) { match e { E::A(x, y) => { print_i32(x); print_f64(y); } E::B(s, n) => { println(s); print_i64(n); } E::C => { println(\"c\"); } } }\nfn main() -> i32 { show(E::A(3, 2.5)); show(E::B(\"hi\", -9000000000)); show(E::C); return 0; }"),
    ("floats", "fn main() -> i32 { print_f64(2.0); print_f64(2.5); print_f64(0.1); print_f64(1.0e21); print_f64(1.0e-7); print_f64(0.0 / 0.0); print_f64(1.0 / 0.0); print_f64(-0.0); println(f64_to_string(123.456)); return 0; }"),
    ("bounds", "fn main() -> i32 { print_i32(1); let a = [1, 2]; let i = 5; print_i32(a[i]); return 0; }"),
    ("div0", "fn main() -> i32 { print_i32(1); let z = 0; print_i32(5 / z); return 0; }"),
    ("minwrap", "fn main() -> i32 { let a = 0 - 2147483647 - 1; let b = 0 - 1; print_i32(a / b); print_i32(a % b); let c: i64 = -9223372036854775808; let d: i64 = -1; print_i64(c / d); return 0; }"),
    ("casts", "fn main() -> i32 { print_i32(3.9e10 as i32); print_i32((0.0 / 0.0) as i32); print_i64(2.9 as i64); print_i32('A' as i32); print_char(66 as char); return 0; }"),
    ("len_array", "fn main() -> i32 { let a = [7; 4]; let z: [i32; 0] = []; print_i32(len(a)); print_i32(len(z)); return 0; }"),
    ("match_array", "enum E { A(i32), B }\nfn f(e: E) -> [i32; 2] { return match e { E::A(n) => [n, n], E::B => [0, 1] }; }\nfn main() -> i32 { let a = f(E::A(4)); print_i32(a[0] + a[1]); print_i32(f(E::B)[1]); return 0; }"),
    ("assert", "fn main() -> i32 { print_i32(1); assert(1 == 2); return 0; }"),
    ("nested", "struct In { v: [i32; 2] }\nstruct Out { i: In, t: (i32, f64) }\nfn main() -> i32 { let mut o = Out { i: In { v: [1, 2] }, t: (3, 4.5) }; let c = o; o.i.v[1] = 9; o.t.0 = 8; print_i32(c.i.v[1]); print_i32(o.i.v[1]); print_i32(c.t.0); print_bool(c == o); return 0; }"),
    ("tuple_eq", "fn main() -> i32 { let a = (1, 0.0 / 0.0); print_bool(a == a); print_bool((1, -0.0) == (1, 0.0)); return 0; }"),
    ("i64_print", "fn main() -> i32 { print_i64(-9223372036854775807 - 1); println(i64_to_string(42)); println(to_string(-7)); return 0; }"),
];

#[test]
fn llvm_matches_vm_on_hand_written_programs() {
    if !have_lli() {
        return;
    }
    let dir = scratch("hand");
    let mut failures = Vec::new();
    for (name, src) in HAND {
        let p = dir.join(format!("{name}.ae"));
        std::fs::write(&p, src).unwrap();
        check(&p, &mut failures);
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for sub in ["examples", "stdlib"] {
        for e in std::fs::read_dir(root.join(sub)).unwrap().flatten() {
            let p = e.path();
            let src = std::fs::read_to_string(&p).unwrap_or_default();
            if p.extension().map(|x| x == "ae").unwrap_or(false) && src.contains("fn main") {
                check(&p, &mut failures);
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(failures.is_empty(), "{} mismatch(es):\n{}", failures.len(), failures.join("\n"));
}

/// Large randomized run: `cargo test --release --test audit_llvm -- --ignored`.
#[test]
#[ignore]
fn llvm_matches_vm_on_generated_programs() {
    if !have_lli() {
        return;
    }
    let n: u64 = std::env::var("AETHER_LLVM_DIFF_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(150);
    let dir = scratch("gen");
    let mut failures = Vec::new();
    for seed in 0..n {
        let mut rng = aether::fuzz::FuzzRng::new(0xC0FFEE + seed);
        for (kind, src) in [
            ("agg", aether::fuzz::gen_aggregate_program(&mut rng)),
            ("lang", aether::fuzz::gen_lang_source(&mut rng)),
        ] {
            let p = dir.join(format!("{kind}_{seed}.ae"));
            std::fs::write(&p, &src).unwrap();
            check(&p, &mut failures);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(failures.is_empty(), "{} mismatch(es):\n{}", failures.len(), failures.join("\n"));
}

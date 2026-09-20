use std::process::Command;

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_aether"));
    c.current_dir(env!("CARGO_MANIFEST_DIR"));
    c
}

#[test]
fn verify_fib() {
    let out = bin().args(["verify", "examples/fib.ae"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn cfg_is_dot() {
    let out = bin().args(["cfg", "examples/fib.ae"]).output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("digraph"));
}

#[test]
fn fmt_emits_fn() {
    let out = bin().args(["fmt", "examples/hello.ae"]).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("fn main"));
}

#[test]
fn opaque_returns_one() {
    let out = bin()
        .args(["run", "examples/opaque.ae", "-O2"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn math_stdlib_runs() {
    let out = bin().args(["run", "stdlib/math.ae"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}


#[test]
fn dump_liveness_mentions_function() {
    let out = bin()
        .args(["dump-liveness", "examples/fib.ae"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("function"));
}

#[test]
fn inline_example_runs() {
    let out = bin()
        .args(["run", "examples/inline.ae", "-O2"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn cse_example_runs() {
    let out = bin()
        .args(["run", "examples/cse.ae", "-O2"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn game_score_runs() {
    let out = bin()
        .args(["run", "examples/game_score.ae", "-O2"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn stdlib_extra_runs() {
    for p in ["stdlib/cmp.ae", "stdlib/loops.ae", "stdlib/bits.ae"] {
        let out = bin().args(["run", p]).output().unwrap();
        assert!(out.status.success(), "{p} {}", String::from_utf8_lossy(&out.stderr));
    }
}

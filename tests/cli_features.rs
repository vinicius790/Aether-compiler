//! Integration tests for the CLI features added in 0.2.2: `--include`,
//! `AETHER_INCLUDE`, VM limits, `bench`, `--backend llvm` and the stateful REPL.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_aether"));
    c.current_dir(env!("CARGO_MANIFEST_DIR"));
    c.env_remove("AETHER_INCLUDE");
    c
}

/// A scratch directory unique to this test (and process) under the target dir.
fn scratch(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("cli_features")
        .join(format!("{test}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &PathBuf, name: &str, src: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, src).unwrap();
    p.to_string_lossy().into_owned()
}

fn path_has(tool: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(tool).is_file()))
        .unwrap_or(false)
}

const LIB: &str = "pub fn twice(x: i32) -> i32 { return x * 2; }\n";
const MAIN: &str = "fn main() -> i32 { print_i32(twice(21)); return 0; }\n";

#[test]
fn include_links_two_files() {
    let d = scratch("include");
    let lib = write(&d, "lib.ae", LIB);
    let main = write(&d, "main.ae", MAIN);
    let out = bin()
        .args(["run", &main, "--include", &lib])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "42\n");

    // without the include the call is unresolved → compile error (exit 1)
    let out = bin().args(["run", &main]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));

    // `check` accepts the include too
    let out = bin()
        .args(["check", &main, "--include", &lib])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn include_env_var_adds_default_includes() {
    let d = scratch("include-env");
    let lib = write(&d, "lib.ae", LIB);
    let main = write(&d, "main.ae", MAIN);
    let out = bin()
        .args(["run", &main])
        .env("AETHER_INCLUDE", format!(":{lib}:"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "42\n");
}

#[test]
fn include_duplicate_names_point_at_the_included_file() {
    let d = scratch("include-dup");
    let lib = write(&d, "lib.ae", LIB);
    let main = write(
        &d,
        "main.ae",
        "fn twice(x: i32) -> i32 { return x + x; }\nfn main() -> i32 { return twice(1); }\n",
    );
    let out = bin()
        .args(["check", &main, "--include", &lib])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("duplicate function `twice`"), "{err}");
    assert!(err.contains("lib.ae"), "{err}");
}

#[test]
fn prelude_is_includable() {
    let d = scratch("prelude");
    let main = write(
        &d,
        "main.ae",
        "fn main() -> i32 {\n    print_i32(gcd(12, 18));\n    print_i32(wrap_index(0 - 1, 5));\n    print_bool(is_even(4));\n    print_i32(sum_to(5));\n    return sign(0 - 9);\n}\n",
    );
    let out = bin()
        .args(["run", &main, "--include", "stdlib/prelude.ae", "--stats"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "6\n4\ntrue\n10\n");
    assert!(String::from_utf8_lossy(&out.stderr).contains("exit = -1"));
}

#[test]
fn max_steps_stops_a_loop_and_keeps_partial_stdout() {
    let d = scratch("max-steps");
    let main = write(
        &d,
        "main.ae",
        "fn main() -> i32 {\n    print_i32(1);\n    let mut i = 0;\n    while i < 1000000 { i = i + 1; }\n    print_i32(2);\n    return 0;\n}\n",
    );
    let out = bin()
        .args(["run", &main, "--max-steps", "10"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("step limit"), "{err}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n");

    // digest honours the same limit
    let out = bin()
        .args(["digest", &main, "--max-steps", "10"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn max_depth_stops_deep_recursion() {
    let d = scratch("max-depth");
    let main = write(
        &d,
        "main.ae",
        "fn down(n: i32) -> i32 { if n == 0 { return 0; } return down(n - 1); }\nfn main() -> i32 { return down(500); }\n",
    );
    let out = bin()
        .args(["run", &main, "--max-depth", "8"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("stack overflow"));
    let out = bin().args(["run", &main]).output().unwrap();
    assert!(out.status.success());
}

#[test]
fn bench_prints_both_levels() {
    let out = bin()
        .args(["bench", "examples/fib.ae", "--n", "2"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("-O0:"), "{s}");
    assert!(s.contains("-O2:"), "{s}");
    assert!(s.contains("vm_steps="), "{s}");
    assert!(s.contains("ir_insts="), "{s}");
    assert!(s.contains("speedup"), "{s}");
}

#[test]
fn backend_llvm_runs_hello_when_lli_is_available() {
    if !(path_has("lli") || path_has("lli-18")) {
        eprintln!("skipping: lli not on PATH");
        return;
    }
    let out = bin()
        .args(["run", "examples/hello.ae", "--backend", "llvm"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("hello, aether"));
}

#[test]
fn backend_llvm_without_lli_is_a_clear_error() {
    let d = scratch("no-lli");
    // An empty PATH hides every tool; `which` itself is then missing too,
    // which run_with_lli reports the same way.
    let out = bin()
        .args(["run", "examples/hello.ae", "--backend", "llvm"])
        .env("PATH", d.to_string_lossy().into_owned())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("lli"), "{err}");
}

#[test]
fn unknown_backend_is_rejected() {
    let out = bin()
        .args(["run", "examples/hello.ae", "--backend", "jit"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown backend"));
}

#[test]
fn repl_keeps_definitions_between_inputs() {
    let mut child = bin()
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"fn sq(x: i32) -> i32 { return x * x; }\n\nprint_i32(sq(7));\n\n:quit\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("defined sq"), "{s}");
    assert!(s.contains("49"), "{s}");
}

#[test]
fn repl_error_keeps_previous_definitions_and_reset_clears() {
    let mut child = bin()
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let script = b"fn sq(x: i32) -> i32 { return x * x; }\n\n\
                   fn bad() -> i32 { return \"x\"; }\n\n\
                   :items\n\
                   print_i32(sq(3));\n\n\
                   :reset\n\
                   :items\n\
                   :quit\n";
    child.stdin.take().unwrap().write_all(script).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("(input discarded; previous definitions kept)"), "{s}");
    assert!(s.contains("9\n"), "{s}");
    assert!(s.contains("(no definitions)"), "{s}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("error"), "{err}");
}

#[test]
fn usage_lists_new_options_and_commands() {
    let out = bin().arg("help").output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    for needle in [
        "--include", "--max-steps", "--max-depth", "--backend", "bench", "profile", "digest",
        "stats", "dump-hir", "dump-liveness",
    ] {
        assert!(s.contains(needle), "usage lacks {needle}");
    }
}

#[test]
fn deeply_nested_program_does_not_overflow_the_stack() {
    let d = scratch("deep");
    let depth = 3000;
    let mut src = String::from("fn main() -> i32 {\n    let x = ");
    for _ in 0..depth {
        src.push('(');
    }
    src.push('1');
    for _ in 0..depth {
        src.push(')');
    }
    src.push_str(";\n    return x;\n}\n");
    let main = write(&d, "deep.ae", &src);
    let out = bin().args(["check", &main]).output().unwrap();
    // Either a clean compile or a diagnostic — never a crash.
    assert!(
        out.status.code().map(|c| c <= 2).unwrap_or(false),
        "status={:?} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

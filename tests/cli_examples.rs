use std::process::Command;

fn aether() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aether"))
}

fn repo_file(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

#[test]
fn run_hello() {
    let out = aether()
        .args(["run", repo_file("examples/hello.ae").to_str().unwrap()])
        .output()
        .expect("run");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hello, aether"), "{stdout}");
}

#[test]
fn run_opt_demo_prints_19() {
    let out = aether()
        .args(["run", repo_file("examples/opt_demo.ae").to_str().unwrap()])
        .output()
        .expect("run");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("19"), "{stdout}");
}

#[test]
fn optimize_reports_delta() {
    let out = aether()
        .args(["optimize", repo_file("examples/opt_demo.ae").to_str().unwrap()])
        .output()
        .expect("optimize");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("IR instructions"), "{stdout}");
}

#[test]
fn check_rejects_bad_program() {
    use std::io::Write;
    let dir = std::env::temp_dir();
    let path = dir.join("aether_bad.ae");
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "fn main() -> i32 {{ return true; }}").unwrap();
    let out = aether()
        .args(["check", path.to_str().unwrap()])
        .output()
        .expect("check");
    assert!(!out.status.success());
}

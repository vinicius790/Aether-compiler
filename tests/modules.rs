//! End-to-end tests for file modules (`use "path";`) through the binary.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_aether"));
    c.current_dir(env!("CARGO_MANIFEST_DIR"));
    c
}

fn run(args: &[&str]) -> Output {
    bin().args(args).output().expect("spawn aether")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Fresh scratch directory under the system temp dir (std only).
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "aether-modules-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, src: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, src).unwrap();
    p.display().to_string()
}

#[test]
fn modules_example_runs_the_same_at_o0_and_o2() {
    let o0 = run(&["run", "examples/modules.ae", "-O0"]);
    assert!(o0.status.success(), "{}", stderr(&o0));
    let o2 = run(&["run", "examples/modules.ae", "-O2"]);
    assert!(o2.status.success(), "{}", stderr(&o2));
    assert_eq!(stdout(&o0), stdout(&o2));
    let text = stdout(&o0);
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    assert_eq!(&lines[..4], &["5", "11", "6", "8"], "{lines:?}");
    assert_eq!(lines.len(), 6, "{lines:?}");
}

#[test]
fn stdlib_prelude_is_importable_from_the_repo_root() {
    let dir = scratch("prelude");
    // the path is relative to the importing file, so point back at the repo
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("stdlib/prelude.ae");
    let main = write(
        &dir,
        "main.ae",
        &format!(
            "use {:?};\nfn main() -> i32 {{ print_i32(gcd(12, 18)); return 0; }}\n",
            root.display().to_string()
        ),
    );
    let o = run(&["run", &main]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stdout(&o).trim(), "6");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn chained_imports_with_a_cycle_compile_each_file_once() {
    let dir = scratch("cycle");
    let a = write(
        &dir,
        "a.ae",
        "use \"b\";\nfn main() -> i32 { print_i32(b() + c()); return 0; }\n",
    );
    write(&dir, "b.ae", "use \"c.ae\";\npub fn b() -> i32 { return 20; }\n");
    write(&dir, "c.ae", "use \"a.ae\";\npub fn c() -> i32 { return 22; }\n");
    let o = run(&["run", &a]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stdout(&o).trim(), "42");
    // `check` agrees, and `dump-ast` lists the three files' items once each
    let o = run(&["check", &a]);
    assert!(o.status.success(), "{}", stderr(&o));
    let o = run(&["dump-ast", &a]);
    assert!(o.status.success(), "{}", stderr(&o));
    let ast = stdout(&o);
    assert_eq!(ast.matches("fn b(").count(), 1, "{ast}");
    assert_eq!(ast.matches("fn c(").count(), 1, "{ast}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_import_fails_with_unresolved_import() {
    let dir = scratch("missing");
    let main = write(
        &dir,
        "main.ae",
        "fn main() -> i32 { return 0; }\nuse \"does/not/exist\";\n",
    );
    let o = run(&["run", &main]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(err.contains("E0280"), "{err}");
    assert!(err.contains("unresolved import"), "{err}");
    assert!(err.contains("does/not/exist.ae"), "{err}");
    assert!(err.contains("main.ae:2"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn check_accepts_pub_items_and_fmt_keeps_them() {
    let dir = scratch("pub");
    let main = write(
        &dir,
        "main.ae",
        "pub struct P { x: i32, }\npub fn get(p: P) -> i32 { return p.x; }\n\
         pub extern fn host(x: i32) -> i32;\nfn main() -> i32 { return get(P { x: 0 }); }\n",
    );
    let o = run(&["check", &main]);
    assert!(o.status.success(), "{}", stderr(&o));
    let o = run(&["fmt", &main]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("pub struct P"), "{out}");
    assert!(out.contains("pub fn get("), "{out}");
    assert!(out.contains("pub extern fn host("), "{out}");
    assert!(out.contains("\nfn main("), "{out}");
    // `pub use` is not a thing
    let bad = write(&dir, "bad.ae", "pub use \"x\";\nfn main() -> i32 { return 0; }\n");
    let o = run(&["check", &bad]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("expected item"), "{}", stderr(&o));
    let _ = std::fs::remove_dir_all(&dir);
}

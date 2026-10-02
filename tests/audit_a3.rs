//! Audit of the CLI, the REPL, diagnostics and the docs that describe them.
//! Every program runs at -O0 and -O2. `docs/diagnostics.md` is checked row
//! by row: each example must produce exactly the documented code and message.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use aether::driver::{compile_source, compile_sources, CompileOptions};

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_aether"));
    c.current_dir(env!("CARGO_MANIFEST_DIR"));
    c.env_remove("AETHER_INCLUDE");
    c.env("NO_COLOR", "1");
    c
}

fn scratch(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("audit_a3")
        .join(format!("{test}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &PathBuf, name: &str, src: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, src).unwrap();
    p.to_string_lossy().into_owned()
}

fn run(args: &[&str]) -> Output {
    bin().args(args).output().unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn opts(level: u8) -> CompileOptions {
    CompileOptions {
        opt_level: level,
        color: false,
    }
}

/// `(code, message, program)` for every table row of docs/diagnostics.md
/// whose example column is a single program in backticks.
fn documented_examples() -> Vec<(String, String, String)> {
    let doc = include_str!("../docs/diagnostics.md");
    let mut rows = Vec::new();
    for line in doc.lines() {
        let Some(rest) = line.strip_prefix("| `") else { continue };
        let cells: Vec<&str> = rest.split(" | ").collect();
        if cells.len() != 3 {
            continue;
        }
        let code = cells[0].trim_end_matches('`');
        let example = cells[2].trim_end_matches(" |").trim();
        if !(code.starts_with('E') || code.starts_with('W'))
            || !example.starts_with('`')
            || !example.ends_with('`')
            || example.starts_with("``")
        {
            continue;
        }
        let program = example[1..example.len() - 1].to_string();
        if program.contains('`') || !(program.contains("fn ")) {
            continue;
        }
        rows.push((code.to_string(), cells[1].to_string(), program));
    }
    rows
}

#[test]
fn diagnostics_doc_examples_match_the_compiler() {
    let rows = documented_examples();
    assert!(rows.len() >= 60, "parsed only {} rows", rows.len());
    for (code, message, program) in rows {
        for level in [0, 2] {
            let c = compile_source("main.ae", &program, &opts(level));
            let found: Vec<String> = c
                .diags
                .iter()
                .map(|d| format!("{}: {}", d.code.unwrap_or("-"), d.message))
                .collect();
            assert!(
                c.diags
                    .iter()
                    .any(|d| d.code == Some(code.as_str()) && d.message == message),
                "{code} `{message}` not produced by `{program}`; got {found:?}"
            );
        }
    }
}

#[test]
fn diagnostics_doc_nesting_and_module_rows() {
    // E0101: `return` with 300 nested parentheses
    let src = format!("fn main() -> i32 {{ return {}1{}; }}", "(".repeat(300), ")".repeat(300));
    let c = compile_source("main.ae", &src, &opts(0));
    assert!(c
        .diags
        .iter()
        .any(|d| d.code == Some("E0101") && d.message == "nesting too deep (limit 256)"));

    let dir = scratch("modrows");
    write(&dir, "main.ae", "use \"missing\";\nfn main() -> i32 { return 0; }\n");
    let out = bin().current_dir(&dir).args(["check", "main.ae"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(
        err.contains("[E0280] unresolved import: cannot read missing.ae: No such file or directory (os error 2) (imported from main.ae:1)"),
        "{err}"
    );

    write(&dir, "p.ae", "fn priv() -> i32 { return 1; }\n");
    write(&dir, "m2.ae", "use \"p\";\nfn main() -> i32 { return priv(); }\n");
    let err = text(&bin().current_dir(&dir).args(["check", "m2.ae"]).output().unwrap().stderr);
    for want in [
        "[E0281] `priv` is private to `p.ae`",
        "function `priv` is defined at p.ae:1:4 without `pub`",
        "mark it `pub` in p.ae",
    ] {
        assert!(err.contains(want), "missing `{want}` in {err}");
    }
}

#[test]
fn every_emitted_code_is_documented() {
    let doc = include_str!("../docs/diagnostics.md");
    let src_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![src_dir];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let s = std::fs::read_to_string(&p).unwrap();
            let b = s.as_bytes();
            for i in 0..b.len().saturating_sub(6) {
                if b[i] == b'"'
                    && (b[i + 1] == b'E' || b[i + 1] == b'W')
                    && b[i + 2..i + 6].iter().all(u8::is_ascii_digit)
                    && b[i + 6] == b'"'
                {
                    let code = &s[i + 1..i + 6];
                    assert!(doc.contains(&format!("`{code}`")), "{code} ({}) missing from docs/diagnostics.md", p.display());
                }
            }
        }
    }
}

#[test]
fn usage_errors_and_help() {
    let none = run(&[]);
    assert_eq!(none.status.code(), Some(1));
    assert!(text(&none.stderr).contains("missing command") && none.stdout.is_empty());

    for args in [&["help"][..], &["run", "--help"], &["check", "-h"], &["--help"]] {
        let o = run(args);
        assert_eq!(o.status.code(), Some(0), "{args:?}");
        assert!(text(&o.stdout).contains("COMMANDS:"), "{args:?}");
    }
    let v = run(&["version"]);
    assert_eq!(v.status.code(), Some(0));
    assert!(text(&v.stdout).contains(env!("CARGO_PKG_VERSION")));

    for args in [
        &["version", "examples/hello.ae"][..],
        &["fuzz", "examples/hello.ae"],
        &["benchmark", "examples/hello.ae"],
        &["repl", "examples/hello.ae"],
        &["check", "examples/hello.ae", "--include="],
        &["frobnicate"],
        &["run", "examples/hello.ae", "-O7"],
    ] {
        let o = run(args);
        assert_eq!(o.status.code(), Some(1), "{args:?}: {}", text(&o.stderr));
        assert!(!text(&o.stderr).contains("panicked"));
    }
}

#[test]
fn benchmark_honours_vm_limits() {
    let o = run(&["benchmark", "--n", "15", "--max-steps", "100"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o.stderr));
    assert!(text(&o.stderr).contains("runtime error at -O0: execution exceeded the instruction step limit"));
    let ok = run(&["benchmark", "--n", "10"]);
    assert_eq!(ok.status.code(), Some(0));
    assert!(text(&ok.stdout).contains("result -O0 = 55"));
}

#[test]
fn dump_tokens_reports_lexical_errors() {
    let dir = scratch("tokens");
    let f = write(&dir, "t.ae", "fn main() -> i32 { let x = 1 @ 2; return 0; }\n");
    let o = run(&["dump-tokens", &f]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stdout).contains("Ident"), "{}", text(&o.stdout));
    assert!(text(&o.stderr).contains("[E0001] unexpected character `@`"));
    let ok = run(&["dump-tokens", "examples/hello.ae"]);
    assert_eq!(ok.status.code(), Some(0));
}

#[test]
fn compile_names_an_unwritable_output() {
    let o = run(&["compile", "examples/hello.ae", "-o", "/nonexistent-dir/x.ir"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("cannot write /nonexistent-dir/x.ir"), "{}", text(&o.stderr));
}

fn repl(input: &str) -> (String, String) {
    let mut child = bin()
        .args(["repl", "-O0"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let o = child.wait_with_output().unwrap();
    (text(&o.stdout), text(&o.stderr))
}

#[test]
fn repl_tail_expression_commands_and_errors() {
    let (out, _) = repl("fn dbl(x: i32) -> i32 { return x * 2; }\n\nlet x = 2; x * dbl(3)\n\n:quit\n");
    assert!(out.contains("=> 12"), "{out}");
    let (_, err) = repl(":bogus\n\n:q\n");
    assert!(err.contains("unknown REPL command `:bogus`"), "{err}");
    let (_, err) = repl("nope(1)\n\n:q\n");
    assert!(err.contains("unknown function `nope`") && !err.contains("expected"), "{err}");
    let (out, _) = repl(":help\n:q\n");
    assert!(out.contains(":items") && out.contains("blank line"), "{out}");
}

#[test]
fn literal_match_adopts_the_other_operand_type() {
    let src = "fn main() -> i32 {\n    let k = 0;\n    let y: i64 = 5;\n    \
               print_bool((match k { 0 => 3000000000, _ => 2 }) > y);\n    \
               print_bool(y < (match k { 0 => 1, 1 => -4, _ => 2 }));\n    \
               print_i64(y + (match k + 1 { 1 => 10, _ => 20 }) * 2);\n    \
               print_i32((match (k, 2) { (0, x) => match x { 2 => 20, _ => 21 }, _ => 0 }) + 1);\n    return 0;\n}\n";
    let dir = scratch("matchlit");
    let f = write(&dir, "m.ae", src);
    for o in ["-O0", "-O2"] {
        let out = run(&["run", &f, o]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        assert_eq!(text(&out.stdout), "true\nfalse\n25\n21\n");
    }
}

#[test]
fn o1_compacts_registers_past_the_vm_limit() {
    // 80 000 single-use temporaries: E0300 at -O1 before the fix
    let elems = vec!["1"; 80_000].join(",");
    let src = format!("fn main() -> i32 {{ let a = [{elems}]; print_i32(a[79999]); return 0; }}\n");
    let dir = scratch("regs");
    let f = write(&dir, "r.ae", &src);
    for o in ["-O0", "-O1", "-O2"] {
        let out = run(&["run", &f, o]);
        assert_eq!(out.status.code(), Some(0), "{o}: {}", text(&out.stderr));
        assert_eq!(text(&out.stdout), "1\n");
    }
}

#[test]
fn compile_sources_does_not_reload_the_main_file_in_a_cycle() {
    let dir = scratch("cycle");
    let main_src = "use \"b\";\npub fn a() -> i32 { return 1; }\nfn main() -> i32 { return a() + b(); }\n";
    let main = write(&dir, "main.ae", main_src);
    write(&dir, "b.ae", "use \"main\";\npub fn b() -> i32 { return a() + 10; }\n");
    for level in [0, 2] {
        let c = compile_sources(vec![(main.clone(), main_src.to_string())], &opts(level));
        let errs: Vec<String> = c.diags.iter().map(|d| d.message.clone()).collect();
        assert!(!c.diags.has_errors(), "{errs:?}");
    }
}

#[test]
fn diagnostics_never_print_raw_control_characters() {
    let dir = scratch("ctl");
    let f = write(&dir, "c.ae", "fn main() -> i32 { let s = \"\u{1b}[2J\"; return s; }\n");
    let o = run(&["check", &f, "--color"]);
    assert_eq!(o.status.code(), Some(1));
    let err = text(&o.stderr);
    assert!(err.contains("E0235") && err.contains('\u{FFFD}'), "{err}");
    assert!(!err.contains("\u{1b}[2J"), "{err:?}");

    // outside a string the ESC is `unexpected character`, quoted escaped
    let f = write(&dir, "d.ae", "fn main() -> i32 { \u{1b}[31m return 0; }\n");
    for o in ["-O0", "-O2"] {
        let out = run(&["run", &f, o]);
        assert_eq!(out.status.code(), Some(1));
        let err = text(&out.stderr);
        assert!(err.contains("[E0001] unexpected character `\\u{1b}`"), "{err}");
        assert!(!err.contains('\u{1b}'), "{err:?}");
    }
}

#[test]
fn llvm_backend_relays_runtime_messages() {
    let has_lli = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("lli").is_file() || d.join("lli-18").is_file()))
        .unwrap_or(false);
    if !has_lli {
        return;
    }
    let dir = scratch("llvm");
    let f = write(
        &dir,
        "o.ae",
        "fn main() -> i32 { let a = [1, 2, 3]; let i = 5; print_i32(7); return a[i]; }\n",
    );
    for o in ["-O0", "-O2"] {
        let out = run(&["run", &f, o, "--backend", "llvm"]);
        assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
        assert_eq!(text(&out.stdout), "7\n");
        assert!(text(&out.stderr).contains("runtime error: array index 5 out of bounds"), "{}", text(&out.stderr));
    }
}

#[test]
fn versions_agree_across_files() {
    let v = env!("CARGO_PKG_VERSION");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap();
    assert!(read("CITATION.cff").contains(&format!("version: {v}\n")));
    assert!(read("README.md").contains(&format!("version-{v}-")));
    assert!(read("MANUAL.md").starts_with(&format!("# Manual do repositório Aether {v}\n")));
    assert!(read("CHANGELOG.md").contains(&format!("## [{v}]")));
    let major_minor: String = v.rsplit_once('.').unwrap().0.to_string();
    assert!(read("SECURITY.md").contains(&format!("| {major_minor}.x ")));
}

#[test]
fn prelude_gcd_is_never_negative_for_i32_min() {
    let src = "fn main() -> i32 {\n    print_i32(gcd(-2147483648, 6));\n    print_i32(gcd(6, -2147483648));\n    \
               print_i32(gcd(-12, 18));\n    print_i32(gcd(-2147483648, -1));\n    print_i32(gcd(0, -5));\n    return 0;\n}\n";
    let dir = scratch("gcd");
    let f = write(&dir, "g.ae", src);
    for o in ["-O0", "-O2"] {
        let out = run(&["run", &f, o, "--include", "stdlib/prelude.ae"]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        assert_eq!(text(&out.stdout), "2\n2\n6\n1\n5\n");
    }
}

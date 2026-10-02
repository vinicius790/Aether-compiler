//! Golden tests: the IR (-O0 and -O2) and the bytecode (-O2) of every example
//! must match `goldens/`. Regenerate after an intentional change with
//! `UPDATE_GOLDENS=1 cargo test --test goldens`.

use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn dump(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_aether"))
        .current_dir(root())
        .args(args)
        .output()
        .expect("run aether");
    assert!(
        out.status.success(),
        "{:?} failed:\n{}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn check(golden: &str, actual: &str) -> Result<(), String> {
    let path = root().join("goldens").join(golden);
    if std::env::var_os("UPDATE_GOLDENS").is_some() {
        std::fs::write(&path, actual).expect("write golden");
        return Ok(());
    }
    let expected = std::fs::read_to_string(&path)
        .map_err(|e| format!("{golden}: cannot read golden ({e}); run with UPDATE_GOLDENS=1"))?;
    if expected == actual {
        Ok(())
    } else {
        Err(format!(
            "{golden} differs from the compiler output; run with UPDATE_GOLDENS=1 if intended"
        ))
    }
}

#[test]
fn examples_match_goldens() {
    let mut sources: Vec<(String, String)> = std::fs::read_dir(root().join("examples"))
        .expect("examples dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "ae").unwrap_or(false))
        .map(|p| {
            let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
            (stem, format!("examples/{}", p.file_name().unwrap().to_string_lossy()))
        })
        .collect();
    sources.push(("math".into(), "stdlib/math.ae".into()));
    sources.sort();

    let mut failures = Vec::new();
    for (stem, rel) in &sources {
        let cases = [
            (format!("{stem}.ir.O0.txt"), dump(&["dump-ir", rel, "-O0"])),
            (format!("{stem}.ir.O2.txt"), dump(&["dump-ir", rel, "-O2"])),
            (format!("{stem}.bc.txt"), dump(&["dump-bytecode", rel, "-O2"])),
        ];
        for (golden, actual) in cases {
            if let Err(e) = check(&golden, &actual) {
                failures.push(e);
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

use std::process::Command;

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_aether"));
    c.current_dir(env!("CARGO_MANIFEST_DIR"));
    c
}

#[test]
fn corpus_sample_runs() {
    let root = format!("{}/corpus", env!("CARGO_MANIFEST_DIR"));
    let mut ok = 0;
    let mut seen = 0;
    for i in [0u32, 3, 7, 11, 20, 40, 80].iter() {
        let path = format!("{root}/c{i:03}.ae");
        if !std::path::Path::new(&path).exists() {
            continue;
        }
        seen += 1;
        let out = bin().args(["run", &path, "-O2"]).output().unwrap();
        if out.status.success() {
            ok += 1;
        }
    }
    assert!(seen >= 3, "expected corpus files");
    assert_eq!(ok, seen, "some corpus programs failed");
}

//! Compiler driver: source → tokens → AST → HIR → IR → opt → backend.

use crate::ast::{dump_program, Item, Program};
use crate::backend::{assemble, emit_llvm_ir, BytecodeModule};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::ir::{dump_ir, emit_ir, IrModule};
use crate::lexer::tokenize;
use crate::opt::{optimize, OptReport};
use crate::parser::parse;
use crate::sema::{analyze_with_files, HirProgram};
use crate::span::{FileId, Session, Span};
use crate::token::{Token, TokenKind};
use crate::vm::{Value, Vm, VmError, VmOptions};
use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Environment variable holding a colon-separated list of files that are
/// included in every compilation (same effect as repeating `--include`).
pub const INCLUDE_ENV: &str = "AETHER_INCLUDE";

/// Largest source file the driver accepts, per file.
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct CompileOptions {
    pub opt_level: u8,
    pub color: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        CompileOptions {
            opt_level: 2,
            color: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StageTimings {
    pub lex_us: u128,
    pub parse_us: u128,
    pub sema_us: u128,
    pub ir_us: u128,
    pub opt_us: u128,
    pub codegen_us: u128,
    pub exec_us: u128,
}

impl StageTimings {
    pub fn render(&self) -> String {
        format!(
            "timings (µs): lex={} parse={} sema={} ir={} opt={} codegen={} exec={}\n",
            self.lex_us,
            self.parse_us,
            self.sema_us,
            self.ir_us,
            self.opt_us,
            self.codegen_us,
            self.exec_us
        )
    }
}

pub struct Compiled {
    pub session: Session,
    /// The main (first) source file; included files follow it in the session.
    pub file: FileId,
    pub tokens: Vec<Token>,
    pub program: Program,
    pub hir: Option<HirProgram>,
    pub ir: Option<IrModule>,
    pub ir_unopt: Option<IrModule>,
    pub bytecode: Option<BytecodeModule>,
    pub llvm: Option<String>,
    pub opt_report: Option<OptReport>,
    pub diags: Diagnostics,
    pub timings: StageTimings,
}

impl Compiled {
    pub fn ok(&self) -> bool {
        !self.diags.has_errors() && self.bytecode.is_some()
    }
}

/// A VM failure together with whatever the program printed before it.
#[derive(Debug)]
pub struct RunError {
    pub error: VmError,
    pub stdout: String,
    pub steps: u64,
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.error)
    }
}

impl std::error::Error for RunError {}

/// Compile a single in-memory source file.
pub fn compile_source(name: &str, source: &str, opts: &CompileOptions) -> Compiled {
    compile_sources(vec![(name.to_string(), source.to_string())], opts)
}

/// Compile several in-memory files as one program. The first entry is the
/// main file; every file gets its own [`FileId`] in the session and is
/// lexed and parsed separately so diagnostics point at the right file, then
/// the items of all files are merged into one flat [`Program`]. `use`
/// imports are resolved transitively before semantic analysis: relative to
/// the file's directory when `name` is the path of an existing file, else
/// (a bare label such as `<repl>`) relative to the current directory.
/// Duplicate item names across files surface through the usual sema errors.
pub fn compile_sources(files: Vec<(String, String)>, opts: &CompileOptions) -> Compiled {
    // a name that is a file on disk counts as already loaded, so a `use`
    // that leads back to it (a cycle through the main file) does not read
    // and compile it a second time
    let mut seen = HashSet::new();
    let units = files
        .into_iter()
        .map(|(name, source)| {
            let path = Path::new(&name);
            let dir = if path.is_file() {
                if let Ok(canonical) = std::fs::canonicalize(path) {
                    seen.insert(canonical);
                }
                path.parent().map(Path::to_path_buf)
            } else {
                None
            };
            Unit { name, source, dir }
        })
        .collect();
    compile_units(units, seen, opts, false)
}

/// Like [`compile_sources`], but every item of every file counts as `pub`
/// (no E0281). For hosts that stitch independent snippets into one program,
/// such as the REPL, where each definition lives in its own pseudo-file.
pub fn compile_sources_public(files: Vec<(String, String)>, opts: &CompileOptions) -> Compiled {
    let units = files
        .into_iter()
        .map(|(name, source)| Unit {
            name,
            source,
            dir: None,
        })
        .collect();
    compile_units(units, HashSet::new(), opts, true)
}

/// One source file waiting to be compiled: display name, text and the
/// directory relative `use` paths resolve against (`None` = cwd).
struct Unit {
    name: String,
    source: String,
    dir: Option<PathBuf>,
}

/// Lex and parse every unit, pulling in the files their `use` items name
/// (depth-first, each file once), then run the rest of the pipeline on
/// the merged item list.
fn compile_units(
    units: Vec<Unit>,
    mut seen: HashSet<PathBuf>,
    opts: &CompileOptions,
    all_public: bool,
) -> Compiled {
    let mut queue: VecDeque<Unit> = units.into();
    if queue.is_empty() {
        queue.push_back(Unit {
            name: "<empty>".to_string(),
            source: String::new(),
            dir: None,
        });
    }
    let mut session = Session::new();
    let mut timings = StageTimings::default();
    let mut diags = Diagnostics::new();
    let mut tokens: Vec<Token> = Vec::new();
    let mut last_eof: Option<Token> = None;
    let mut file = FileId::DUMMY;
    let mut program = Program {
        items: Vec::new(),
        span: Span::DUMMY,
    };

    while let Some(unit) = queue.pop_front() {
        let id = session.add_file(unit.name.clone(), unit.source);
        if file == FileId::DUMMY {
            file = id;
        }
        let src = &session.file(id).expect("file just added").source;
        let t0 = Instant::now();
        let (toks, lex_diags) = tokenize(id, src);
        timings.lex_us += t0.elapsed().as_micros();
        diags.extend(lex_diags);

        let t1 = Instant::now();
        let (parsed, parse_diags) = parse(toks.clone());
        timings.parse_us += t1.elapsed().as_micros();
        diags.extend(parse_diags);

        // concatenated token stream for `dump-tokens`: one Eof, last
        last_eof = toks.last().filter(|t| t.kind == TokenKind::Eof).cloned();
        tokens.extend(toks.into_iter().filter(|t| t.kind != TokenKind::Eof));

        let mut imported = Vec::new();
        for item in &parsed.items {
            if let Item::Use(u) = item {
                match load_import(unit.dir.as_deref(), &u.path, &mut seen) {
                    Ok(Some(dep)) => imported.push(dep),
                    Ok(None) => {} // already part of the program
                    Err(msg) => diags.push(
                        Diagnostic::error(
                            format!(
                                "unresolved import: {msg} (imported from {}:{})",
                                unit.name, u.span.line
                            ),
                            u.span,
                        )
                        .with_code("E0280")
                        .with_help("paths are relative to the importing file; `.ae` is optional"),
                    ),
                }
            }
        }
        // depth-first: the imports of this file come before the rest of the queue
        for dep in imported.into_iter().rev() {
            queue.push_front(dep);
        }

        if program.span.is_dummy() {
            program.span = parsed.span;
        }
        let mut parsed = parsed;
        if all_public {
            for item in &mut parsed.items {
                match item {
                    Item::Fn(f) => f.is_pub = true,
                    Item::Struct(s) => s.is_pub = true,
                    Item::Extern(e) => e.is_pub = true,
                    Item::Enum(e) => e.is_pub = true,
                    Item::Use(_) => {}
                }
            }
        }
        program.items.extend(parsed.items);
    }
    tokens.extend(last_eof);

    if diags.has_errors() {
        return Compiled {
            session,
            file,
            tokens,
            program,
            hir: None,
            ir: None,
            ir_unopt: None,
            bytecode: None,
            llvm: None,
            opt_report: None,
            diags,
            timings,
        };
    }

    let t2 = Instant::now();
    let file_names: Vec<String> = session.files().iter().map(|f| f.name.clone()).collect();
    let (hir, sema_diags) = analyze_with_files(&program, file_names);
    timings.sema_us = t2.elapsed().as_micros();
    diags.extend(sema_diags);

    if hir.is_none() {
        return Compiled {
            session,
            file,
            tokens,
            program,
            hir,
            ir: None,
            ir_unopt: None,
            bytecode: None,
            llvm: None,
            opt_report: None,
            diags,
            timings,
        };
    }

    let t3 = Instant::now();
    let ir_unopt = emit_ir(hir.as_ref().unwrap());
    timings.ir_us = t3.elapsed().as_micros();

    let t4 = Instant::now();
    let (ir, report) = optimize(ir_unopt.clone(), opts.opt_level);
    timings.opt_us = t4.elapsed().as_micros();

    let t5 = Instant::now();
    let bytecode = match assemble(&ir) {
        Ok(bc) => Some(bc),
        Err(msg) => {
            let span = assemble_error_span(&ir, &msg);
            diags.push(Diagnostic::error(msg, span).with_code("E0300"));
            None
        }
    };
    let llvm = emit_llvm_ir(&ir);
    timings.codegen_us = t5.elapsed().as_micros();

    Compiled {
        session,
        file,
        tokens,
        program,
        hir,
        ir: Some(ir),
        ir_unopt: Some(ir_unopt),
        bytecode,
        llvm: Some(llvm),
        opt_report: Some(report),
        diags,
        timings,
    }
}

/// Where `use "path"` written in a file inside `from_dir` (`None` = the
/// current directory) points: `.ae` is appended when missing and `.`/`..`
/// components are folded lexically, so `examples/../stdlib/vec2` becomes
/// `stdlib/vec2.ae`.
pub fn resolve_import_path(from_dir: Option<&Path>, path: &str) -> PathBuf {
    let mut rel = path.to_string();
    if !rel.ends_with(".ae") {
        rel.push_str(".ae");
    }
    let joined = match from_dir {
        Some(d) if !d.as_os_str().is_empty() => d.join(&rel),
        _ => PathBuf::from(&rel),
    };
    let mut out = PathBuf::new();
    for comp in joined.components() {
        use std::path::Component::*;
        match comp {
            CurDir => {}
            ParentDir => {
                if matches!(out.components().next_back(), Some(Normal(_))) {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// Load the file a `use` names, unless its canonical path is already in
/// `seen` (then `Ok(None)`: a file imported twice or cyclically is compiled
/// once). Errors are plain messages; the caller attaches the span.
fn load_import(
    from_dir: Option<&Path>,
    path: &str,
    seen: &mut HashSet<PathBuf>,
) -> Result<Option<Unit>, String> {
    if path.trim().is_empty() {
        return Err("empty import path".to_string());
    }
    let full = resolve_import_path(from_dir, path);
    let name = full.display().to_string();
    let canonical = std::fs::canonicalize(&full).map_err(|e| format!("cannot read {name}: {e}"))?;
    if !seen.insert(canonical) {
        return Ok(None);
    }
    let source = read_source(&name)?;
    Ok(Some(Unit {
        dir: full.parent().map(Path::to_path_buf),
        name,
        source,
    }))
}

/// Assembler errors name the offending function in backticks
/// ("function `f` needs ..."); point the diagnostic at its definition
/// when that function exists, else at nothing.
fn assemble_error_span(ir: &IrModule, msg: &str) -> Span {
    let mut parts = msg.split('`');
    parts.next();
    parts
        .next()
        .and_then(|name| ir.functions.iter().find(|f| f.name == name))
        .map(|f| f.span)
        .unwrap_or(Span::DUMMY)
}

/// Compile one file from disk, following its `use` imports.
pub fn compile_file(path: &str, opts: &CompileOptions) -> Result<Compiled, String> {
    compile_files(path, &[], opts)
}

/// Compile `main` together with `includes` (all paths on disk) as one
/// program. Files are read in order; a missing or oversized file is an
/// error before anything is compiled. Includes behave like implicit `use`s
/// of the main file: a file named twice (or also imported) is compiled
/// once. `use` imports are resolved relative to the importing file; a
/// missing import is diagnostic E0280 in the result, not an `Err`.
pub fn compile_files(
    main: &str,
    includes: &[String],
    opts: &CompileOptions,
) -> Result<Compiled, String> {
    let mut seen = HashSet::new();
    let mut units = Vec::with_capacity(includes.len() + 1);
    for path in std::iter::once(main).chain(includes.iter().map(String::as_str)) {
        let source = read_source(path)?;
        if let Ok(canonical) = std::fs::canonicalize(path) {
            if !seen.insert(canonical) {
                continue;
            }
        }
        units.push(Unit {
            name: path.to_string(),
            source,
            dir: Path::new(path).parent().map(Path::to_path_buf),
        });
    }
    Ok(compile_units(units, seen, opts, false))
}

fn read_source(path: &str) -> Result<String, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let too_big = || format!("refusing to compile {path}: file is larger than 8 MiB");
    if let Ok(meta) = file.metadata() {
        if meta.len() > MAX_FILE_BYTES as u64 {
            return Err(too_big());
        }
    }
    // never buffer more than the limit, whatever the file claims to be
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(too_big());
    }
    let mut source = String::from_utf8(bytes).map_err(|e| {
        format!(
            "cannot read {path}: file is not valid UTF-8 (first bad byte at offset {})",
            e.utf8_error().valid_up_to()
        )
    })?;
    // a UTF-8 byte order mark is not part of the program
    if source.starts_with('\u{feff}') {
        source.drain(..'\u{feff}'.len_utf8());
    }
    Ok(source)
}

/// Include paths taken from the `AETHER_INCLUDE` environment variable
/// (colon-separated; empty entries are skipped).
pub fn default_includes() -> Vec<String> {
    std::env::var(INCLUDE_ENV)
        .map(|v| parse_include_list(&v))
        .unwrap_or_default()
}

/// Split a colon-separated include list, dropping blank entries.
pub fn parse_include_list(list: &str) -> Vec<String> {
    list.split(':')
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
        .collect()
}

/// Thread-safe byte sink for the VM's stdout; the buffer stays readable
/// after the VM is dropped so partial output survives a runtime error.
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Run the compiled program on the VM with default limits.
pub fn run_compiled(c: &mut Compiled) -> Result<(Value, String, u64), RunError> {
    run_compiled_with(c, VmOptions::default())
}

/// Run the compiled program on the VM with explicit [`VmOptions`] (step and
/// call-depth limits, tracing). Stdout is captured; on failure the partial
/// output and the step count are returned inside [`RunError`].
pub fn run_compiled_with(
    c: &mut Compiled,
    vm_opts: VmOptions,
) -> Result<(Value, String, u64), RunError> {
    let Some(bc) = c.bytecode.as_ref() else {
        return Err(RunError {
            error: VmError::MissingMain,
            stdout: String::new(),
            steps: 0,
        });
    };
    let slot = Arc::new(Mutex::new(Vec::new()));
    let t = Instant::now();
    let mut vm = Vm::new(bc, vm_opts).with_stdout(Box::new(SharedSink(slot.clone())));
    let result = vm.run();
    let steps = vm.steps();
    c.timings.exec_us = t.elapsed().as_micros();
    let stdout = String::from_utf8_lossy(&slot.lock().unwrap()).into_owned();
    match result {
        Ok(val) => Ok((val, stdout, steps)),
        Err(error) => Err(RunError {
            error,
            stdout,
            steps,
        }),
    }
}

/// How the `lli` process that ran a program ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LliStatus {
    /// Exit status of the process. Under `lli` this is the value `main`
    /// returned, truncated to 8 bits by the operating system.
    Exited(i32),
    /// Killed by a signal (`abort()` from a failed `assert`, a division by
    /// zero or an out-of-bounds index raises SIGABRT = 6).
    Signaled(i32),
    /// Still running when the wall-clock limit passed; `lli` was killed.
    TimedOut,
}

/// What `lli` printed and how it ended.
#[derive(Debug, Clone)]
pub struct LliRun {
    pub stdout: String,
    pub stderr: String,
    pub status: LliStatus,
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Feed `input` to `cmd` on stdin (from a thread, so a large module cannot
/// deadlock against a full output pipe) and collect its output.
fn pipe_through(
    mut cmd: std::process::Command,
    input: &str,
) -> Result<std::process::Output, String> {
    use std::process::Stdio;
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("no stdin pipe")?;
    let data = input.as_bytes().to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&data);
    });
    let out = child.wait_with_output().map_err(|e| e.to_string());
    let _ = writer.join();
    out
}

/// Run textual LLVM IR through `opt -O2` (when installed) and `lli`.
///
/// Unlike [`crate::backend::llvm::run_with_lli`] this keeps the program's
/// stdout and its exit status apart: under `lli` the value returned by `main`
/// is the process exit status, so a non-zero status is a result, not a
/// failure. `Err` means the toolchain itself failed (no `lli`, or `opt`
/// rejecting the module, with its message).
pub fn run_llvm_ir(ir: &str) -> Result<LliRun, String> {
    run_llvm_ir_with(ir, None)
}

/// Feed `input` to `cmd`, collect stdout/stderr on helper threads (so no
/// pipe can fill up and deadlock) and poll the child; past `timeout` it is
/// killed and `None` is returned as its status.
fn run_with_deadline(
    mut cmd: std::process::Command,
    input: &str,
    timeout: std::time::Duration,
) -> Result<(Option<std::process::ExitStatus>, Vec<u8>, Vec<u8>), String> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("no stdin pipe")?;
    let data = input.as_bytes().to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&data);
    });
    let mut out_pipe = child.stdout.take().ok_or("no stdout pipe")?;
    let mut err_pipe = child.stderr.take().ok_or("no stderr pipe")?;
    let out_reader = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = out_pipe.read_to_end(&mut v);
        v
    });
    let err_reader = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = err_pipe.read_to_end(&mut v);
        v
    });
    let start = Instant::now();
    let mut nap = std::time::Duration::from_millis(1);
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(st) => break Some(st),
            None if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            None => {
                std::thread::sleep(nap);
                nap = (nap * 2).min(std::time::Duration::from_millis(25));
            }
        }
    };
    let _ = writer.join();
    let out = out_reader.join().unwrap_or_default();
    let err = err_reader.join().unwrap_or_default();
    Ok((status, out, err))
}

/// [`run_llvm_ir`] with a wall-clock limit on `lli` (`None`: no limit):
/// past it the process is killed and the status is [`LliStatus::TimedOut`],
/// with whatever it printed so far.
pub fn run_llvm_ir_with(ir: &str, timeout: Option<std::time::Duration>) -> Result<LliRun, String> {
    let tool = |names: &[&str]| names.iter().find_map(|n| find_on_path(n));
    let lli = tool(&["lli-18", "lli"]).ok_or_else(|| "lli not found on PATH".to_string())?;
    let processed = match tool(&["opt-18", "opt"]) {
        Some(opt) => {
            let mut cmd = std::process::Command::new(opt);
            cmd.args(["-S", "-O2"]);
            let out = pipe_through(cmd, ir)?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim_end().to_string());
            }
            String::from_utf8_lossy(&out.stdout).into_owned()
        }
        None => ir.to_string(),
    };
    let lli_cmd = std::process::Command::new(lli);
    let (status, stdout, stderr) = match timeout {
        Some(t) => run_with_deadline(lli_cmd, &processed, t)?,
        None => {
            let out = pipe_through(lli_cmd, &processed)?;
            (Some(out.status), out.stdout, out.stderr)
        }
    };
    let Some(status) = status else {
        return Ok(LliRun {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            status: LliStatus::TimedOut,
        });
    };
    let out = std::process::Output { status, stdout, stderr };
    let status = match out.status.code() {
        Some(c) => LliStatus::Exited(c),
        None => {
            #[cfg(unix)]
            let sig = std::os::unix::process::ExitStatusExt::signal(&out.status).unwrap_or(0);
            #[cfg(not(unix))]
            let sig = 0;
            LliStatus::Signaled(sig)
        }
    };
    Ok(LliRun {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        status,
    })
}

/// Number of IR instructions in the optimized module (0 without IR).
pub fn ir_inst_count(c: &Compiled) -> usize {
    c.ir
        .as_ref()
        .map(|m| {
            m.functions
                .iter()
                .map(|f| f.blocks.iter().map(|b| b.insts.len()).sum::<usize>())
                .sum()
        })
        .unwrap_or(0)
}

pub fn dump_tokens(c: &Compiled) -> String {
    let mut s = String::new();
    for t in &c.tokens {
        s.push_str(&format!(
            "{:<12} {:<16} {}:{} {:?}\n",
            format!("{:?}", t.kind),
            t.lexeme,
            t.span.line,
            t.span.column,
            t.span
        ));
    }
    s
}

pub fn dump_ast(c: &Compiled) -> String {
    dump_program(&c.program)
}

pub fn dump_ir_text(c: &Compiled, unopt: bool) -> String {
    let ir = if unopt {
        c.ir_unopt.as_ref()
    } else {
        c.ir.as_ref()
    };
    match ir {
        Some(m) => dump_ir(m),
        None => "<no IR>\n".into(),
    }
}

pub fn dump_bc(c: &Compiled) -> String {
    match &c.bytecode {
        Some(bc) => bc.disassemble(),
        None => "<no bytecode>\n".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_error_carries_partial_stdout() {
        let src = "fn main() -> i32 { print_i32(1); let z = 0; print_i32(5 / z); return 0; }";
        let mut c = compile_source("t.ae", src, &CompileOptions::default());
        assert!(c.ok());
        let e = run_compiled(&mut c).unwrap_err();
        assert_eq!(e.stdout, "1\n");
        assert_eq!(e.to_string(), "division by zero");
        assert!(e.steps > 0);
    }

    #[test]
    fn extern_without_impl_is_runtime_error() {
        let src = r#"extern fn foo(x: string); fn main() -> i32 { foo("x"); return 0; }"#;
        let mut c = compile_source("t.ae", src, &CompileOptions::default());
        assert!(c.ok(), "{}", c.diags.render(&c.session, false));
        let e = run_compiled(&mut c).unwrap_err();
        assert!(e.to_string().contains("extern"), "{e}");
        assert_eq!(e.stdout, "");
    }

    #[test]
    fn compile_sources_links_items_across_files() {
        let files = vec![
            (
                "main.ae".to_string(),
                "fn main() -> i32 { print_i32(twice(21)); return 0; }".to_string(),
            ),
            (
                "lib.ae".to_string(),
                "pub fn twice(x: i32) -> i32 { return x * 2; }".to_string(),
            ),
        ];
        let mut c = compile_sources(files, &CompileOptions::default());
        assert!(c.ok(), "{}", c.diags.render(&c.session, false));
        assert_eq!(c.file, FileId(0));
        assert_eq!(c.session.files().len(), 2);
        // exactly one Eof survives the concatenation, and it is last
        let eofs = c.tokens.iter().filter(|t| t.kind == TokenKind::Eof).count();
        assert_eq!(eofs, 1);
        assert_eq!(c.tokens.last().unwrap().kind, TokenKind::Eof);
        let (v, out, _) = run_compiled(&mut c).unwrap();
        assert_eq!(out, "42\n");
        assert_eq!(v, Value::I32(0));
    }

    #[test]
    fn compile_sources_diagnostics_name_the_right_file() {
        let files = vec![
            ("main.ae".to_string(), "fn main() -> i32 { return 0; }".to_string()),
            (
                "lib.ae".to_string(),
                "fn broken() -> i32 { return \"s\"; }".to_string(),
            ),
        ];
        let c = compile_sources(files, &CompileOptions::default());
        assert!(c.diags.has_errors());
        let text = c.diags.render(&c.session, false);
        assert!(text.contains("lib.ae"), "{text}");
        assert!(!text.contains("main.ae"), "{text}");
    }

    #[test]
    fn compile_sources_reports_duplicates_across_files() {
        let files = vec![
            ("a.ae".to_string(), "fn f() -> i32 { return 1; }\nfn main() -> i32 { return f(); }".to_string()),
            // a `pub` item clashes with any other item of its name
            ("b.ae".to_string(), "pub fn f() -> i32 { return 2; }".to_string()),
        ];
        let c = compile_sources(files, &CompileOptions::default());
        let text = c.diags.render(&c.session, false);
        assert!(text.contains("duplicate function `f`"), "{text}");
        assert!(text.contains("b.ae"), "{text}");
    }

    #[test]
    fn private_items_of_another_file_are_e0281_unless_public_mode() {
        let files = || {
            vec![
                (
                    "main.ae".to_string(),
                    "fn main() -> i32 { print_i32(twice(21)); return 0; }".to_string(),
                ),
                (
                    "lib.ae".to_string(),
                    "fn twice(x: i32) -> i32 { return x * 2; }".to_string(),
                ),
            ]
        };
        let c = compile_sources(files(), &CompileOptions::default());
        let d = c.diags.iter().find(|d| d.code == Some("E0281")).expect("E0281");
        assert_eq!(d.message, "`twice` is private to `lib.ae`");
        assert_eq!(d.help.as_deref(), Some("mark it `pub` in lib.ae"));
        assert_eq!(d.span.file, FileId(0));
        let mut c = compile_sources_public(files(), &CompileOptions::default());
        assert!(c.ok(), "{}", c.diags.render(&c.session, false));
        assert_eq!(run_compiled(&mut c).unwrap().1, "42\n");
    }

    #[test]
    fn bom_and_invalid_utf8_are_handled_when_reading() {
        let dir = scratch("bom");
        let bom = dir.join("bom.ae");
        std::fs::write(&bom, b"\xef\xbb\xbffn main() -> i32 { return 0; }").unwrap();
        let c = compile_file(bom.to_str().unwrap(), &CompileOptions::default()).unwrap();
        assert!(c.ok(), "{}", c.diags.render(&c.session, false));
        let bad = dir.join("bad.ae");
        std::fs::write(&bad, b"ab\xff").unwrap();
        let e = compile_file(bad.to_str().unwrap(), &CompileOptions::default()).err().unwrap();
        assert!(e.contains("not valid UTF-8") && e.contains("offset 2"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_compiled_with_honours_step_limit_and_keeps_stdout() {
        let src = "fn main() -> i32 { print_i32(7); let mut i = 0; while i < 100000 { i = i + 1; } return 0; }";
        let mut c = compile_source("t.ae", src, &CompileOptions::default());
        assert!(c.ok());
        let opts = VmOptions {
            max_steps: 10,
            ..VmOptions::default()
        };
        let e = run_compiled_with(&mut c, opts).unwrap_err();
        assert!(matches!(e.error, VmError::StepLimit), "{e}");
        assert_eq!(e.stdout, "7\n");
        assert_eq!(e.steps, 11);
    }

    #[test]
    fn run_compiled_with_honours_depth_limit() {
        let src = "fn down(n: i32) -> i32 { if n == 0 { return 0; } return down(n - 1); }\nfn main() -> i32 { return down(100); }";
        let mut c = compile_source("t.ae", src, &CompileOptions::default());
        assert!(c.ok());
        let opts = VmOptions {
            max_call_depth: 5,
            ..VmOptions::default()
        };
        let e = run_compiled_with(&mut c, opts).unwrap_err();
        assert!(matches!(e.error, VmError::StackOverflow), "{e}");
        assert!(ir_inst_count(&c) > 0);
    }

    #[test]
    fn include_list_splits_on_colons() {
        assert_eq!(
            parse_include_list("a.ae::b.ae: "),
            vec!["a.ae".to_string(), "b.ae".to_string()]
        );
        assert!(parse_include_list("").is_empty());
    }

    #[test]
    fn assemble_failure_becomes_e0300() {
        // Compile normally, then re-assemble a module whose register count
        // exceeds the VM limit to exercise the diagnostic path end to end.
        let src = "fn main() -> i32 { return 0; }";
        let c = compile_source("t.ae", src, &CompileOptions::default());
        let mut ir = c.ir.clone().unwrap();
        ir.functions[0].reg_count = 70_000;
        let msg = assemble(&ir).unwrap_err();
        let span = assemble_error_span(&ir, &msg);
        assert_eq!(span, ir.functions[0].span);
        let d = Diagnostic::error(msg, span).with_code("E0300");
        assert_eq!(d.code, Some("E0300"));
        assert_eq!(assemble_error_span(&ir, "unknown function `ghost`"), Span::DUMMY);
    }

    /// Fresh scratch directory under the system temp dir (std only).
    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aether-driver-{tag}-{}-{}",
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
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, src).unwrap();
        p.display().to_string()
    }

    #[test]
    fn use_imports_items_from_another_file() {
        let dir = scratch("use");
        write(&dir, "lib.ae", "pub fn twice(x: i32) -> i32 { return x * 2; }");
        let main = write(
            &dir,
            "main.ae",
            "use \"lib\";\nfn main() -> i32 { print_i32(twice(21)); return 0; }",
        );
        let mut c = compile_file(&main, &CompileOptions::default()).unwrap();
        assert!(c.ok(), "{}", c.diags.render(&c.session, false));
        assert_eq!(c.session.files().len(), 2);
        assert!(c.session.file_name(FileId(1)).ends_with("lib.ae"));
        let (_, out, _) = run_compiled(&mut c).unwrap();
        assert_eq!(out, "42\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn use_dedupes_and_tolerates_cycles() {
        let dir = scratch("cycle");
        write(&dir, "a.ae", "use \"b.ae\";\nuse \"c.ae\";\npub fn a() -> i32 { return 1; }");
        write(&dir, "b.ae", "use \"c.ae\";\nuse \"a.ae\";\npub fn b() -> i32 { return 2; }");
        write(&dir, "c.ae", "use \"./a.ae\";\npub fn c() -> i32 { return 3; }");
        let main = write(
            &dir,
            "main.ae",
            "use \"a\";\nuse \"a.ae\";\nfn main() -> i32 { print_i32(a() + b() + c()); return 0; }",
        );
        let mut c = compile_file(&main, &CompileOptions::default()).unwrap();
        assert!(c.ok(), "{}", c.diags.render(&c.session, false));
        // main, a, b, c: each exactly once despite the cycle and the repeats
        assert_eq!(c.session.files().len(), 4);
        let (_, out, _) = run_compiled(&mut c).unwrap();
        assert_eq!(out, "6\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn include_and_use_of_the_same_file_compile_it_once() {
        let dir = scratch("inc");
        let lib = write(&dir, "lib.ae", "pub fn one() -> i32 { return 1; }");
        let main = write(&dir, "main.ae", "use \"lib.ae\";\nfn main() -> i32 { return one(); }");
        let c = compile_files(&main, &[lib.clone(), lib], &CompileOptions::default()).unwrap();
        assert!(c.ok(), "{}", c.diags.render(&c.session, false));
        assert_eq!(c.session.files().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_import_is_e0280_at_the_use_span() {
        let dir = scratch("missing");
        let main = write(
            &dir,
            "main.ae",
            "fn main() -> i32 { return 0; }\nuse \"nowhere/none\";\n",
        );
        let c = compile_file(&main, &CompileOptions::default()).unwrap();
        assert!(!c.ok());
        let d = c.diags.iter().find(|d| d.code == Some("E0280")).expect("E0280");
        assert!(d.message.starts_with("unresolved import: cannot read "), "{}", d.message);
        assert!(d.message.contains("nowhere/none.ae"), "{}", d.message);
        assert!(d.message.contains("main.ae:2)"), "{}", d.message);
        assert_eq!(d.span.file, c.file);
        assert_eq!(d.span.line, 2);
        assert_eq!(d.span.column, 1);
        assert_eq!(d.span.len(), "use \"nowhere/none\";".len() as u32);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn imports_resolve_relative_to_the_importing_file() {
        let dir = scratch("nested");
        write(&dir, "lib/util.ae", "use \"../lib/deep/inner.ae\";\npub fn util() -> i32 { return inner() + 1; }");
        write(&dir, "lib/deep/inner.ae", "pub fn inner() -> i32 { return 41; }");
        let main = write(
            &dir,
            "app/main.ae",
            "use \"../lib/util\";\nfn main() -> i32 { print_i32(util()); return 0; }",
        );
        let mut c = compile_file(&main, &CompileOptions::default()).unwrap();
        assert!(c.ok(), "{}", c.diags.render(&c.session, false));
        // names are folded lexically: no `app/../lib` in the session
        assert!(c.session.files().iter().all(|f| !f.name.contains("/../")));
        let (_, out, _) = run_compiled(&mut c).unwrap();
        assert_eq!(out, "42\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compile_source_resolves_imports_against_cwd() {
        // the repo root is cargo's cwd for unit tests
        let src = "use \"stdlib/prelude\";\nfn main() -> i32 { print_i32(gcd(12, 18)); return 0; }";
        let mut c = compile_source("mem.ae", src, &CompileOptions::default());
        assert!(c.ok(), "{}", c.diags.render(&c.session, false));
        assert_eq!(c.session.file_name(FileId(1)), "stdlib/prelude.ae");
        let (_, out, _) = run_compiled(&mut c).unwrap();
        assert_eq!(out, "6\n");
    }

    #[test]
    fn resolve_import_path_folds_components_and_adds_extension() {
        let p = |d: Option<&str>, s: &str| resolve_import_path(d.map(Path::new), s).display().to_string();
        assert_eq!(p(Some("examples"), "../stdlib/vec2.ae"), "stdlib/vec2.ae");
        assert_eq!(p(Some("examples"), "vec2"), "examples/vec2.ae");
        assert_eq!(p(Some(""), "./x"), "x.ae");
        assert_eq!(p(None, "a/./b/../c"), "a/c.ae");
        assert_eq!(p(Some("a"), "../../up"), "../up.ae");
    }
}

//! Register virtual machine for Aether bytecode.
//!
//! The VM owns its frame stack, so execution can be sliced into budgets
//! ([`Vm::run_budget`]) and resumed; [`Vm::run`] is the "run to completion"
//! wrapper. `extern fn` declarations are bound at runtime through
//! [`Vm::with_host_fn`] / [`HostFn`].

use crate::backend::bytecode::{BcFunction, BytecodeModule, CmpOp, Immediate, Op, MAX_ARRAY_LEN};
use crate::ty::Type;
use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{self, Write};
use std::ops::Deref;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// Immutable, shared string value (`Value::Str`).
///
/// `Clone` shares the text (O(1)), so moving a string between registers or
/// passing it to a function never copies it. `len` and `s[i]` count Unicode
/// scalar values: for ASCII text (checked once, when the string is built)
/// both are O(1); otherwise the first indexed access builds a char-offset
/// table that later accesses reuse (amortised O(1)). Concatenation copies
/// (O(n)), except that the VM appends in place when the left operand's text
/// is not shared and the result overwrites it (`s = s + t` in a loop).
#[derive(Clone)]
pub struct Str(Rc<StrBuf>);

struct StrBuf {
    text: String,
    ascii: bool,
    /// Byte offset of every char, built lazily for non-ASCII text.
    offsets: OnceCell<Box<[u32]>>,
}

impl Str {
    pub fn new(text: String) -> Str {
        let ascii = text.is_ascii();
        Str(Rc::new(StrBuf { text, ascii, offsets: OnceCell::new() }))
    }

    pub fn as_str(&self) -> &str {
        &self.0.text
    }

    /// True when every char is ASCII (so byte and char indices coincide).
    pub fn is_ascii(&self) -> bool {
        self.0.ascii
    }

    fn offsets(&self) -> &[u32] {
        self.0
            .offsets
            .get_or_init(|| self.0.text.char_indices().map(|(i, _)| i as u32).collect())
    }

    /// Number of chars (Unicode scalar values).
    pub fn char_len(&self) -> usize {
        if self.0.ascii {
            self.0.text.len()
        } else {
            self.offsets().len()
        }
    }

    /// The `i`-th char, if any.
    pub fn char_at(&self, i: usize) -> Option<char> {
        if self.0.ascii {
            return self.0.text.as_bytes().get(i).map(|b| *b as char);
        }
        let off = *self.offsets().get(i)? as usize;
        self.0.text[off..].chars().next()
    }

    /// Appends `tail` in place when this handle is the only owner of its
    /// text; returns `false` (and changes nothing) when the text is shared.
    fn try_push_str(&mut self, tail: &Str) -> bool {
        match Rc::get_mut(&mut self.0) {
            Some(buf) => {
                buf.text.push_str(&tail.0.text);
                buf.ascii &= tail.0.ascii;
                buf.offsets = OnceCell::new();
                true
            }
            None => false,
        }
    }
}

impl Deref for Str {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0.text
    }
}

impl AsRef<str> for Str {
    fn as_ref(&self) -> &str {
        &self.0.text
    }
}

impl From<&str> for Str {
    fn from(s: &str) -> Str {
        Str::new(s.to_string())
    }
}

impl From<String> for Str {
    fn from(s: String) -> Str {
        Str::new(s)
    }
}

impl PartialEq for Str {
    fn eq(&self, other: &Str) -> bool {
        Rc::ptr_eq(&self.0, &other.0) || self.0.text == other.0.text
    }
}

impl Eq for Str {}

impl PartialEq<str> for Str {
    fn eq(&self, other: &str) -> bool {
        self.0.text == other
    }
}

impl PartialEq<&str> for Str {
    fn eq(&self, other: &&str) -> bool {
        self.0.text == *other
    }
}

impl PartialOrd for Str {
    fn partial_cmp(&self, other: &Str) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Str {
    fn cmp(&self, other: &Str) -> std::cmp::Ordering {
        self.0.text.cmp(&other.0.text)
    }
}

impl std::hash::Hash for Str {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.0.text.hash(h)
    }
}

impl fmt::Debug for Str {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for Str {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    I32(i32),
    I64(i64),
    F64(f64),
    Bool(bool),
    /// Shared immutable text; see [`Str`] (clone is O(1)).
    Str(Str),
    Char(char),
    Unit,
    /// Copy-on-write aggregate: `Clone` shares the buffer (O(1)); writers go
    /// through [`Rc::make_mut`] and copy only while the buffer is shared.
    Array(Rc<Vec<Value>>),
    /// Struct fields, same copy-on-write scheme as `Array`.
    Object(Rc<Vec<Value>>),
}

impl Value {
    /// Wraps `xs` as an array value.
    pub fn array(xs: Vec<Value>) -> Value { Value::Array(Rc::new(xs)) }

    /// Wraps `fields` as a struct value.
    pub fn object(fields: Vec<Value>) -> Value { Value::Object(Rc::new(fields)) }

    /// Wraps `s` as a string value.
    pub fn str(s: impl Into<Str>) -> Value { Value::Str(s.into()) }

    /// Source-level type name, used in runtime error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::I32(_) => "i32",
            Value::I64(_) => "i64",
            Value::F64(_) => "f64",
            Value::Bool(_) => "bool",
            Value::Str(_) => "string",
            Value::Char(_) => "char",
            Value::Unit => "unit",
            Value::Array(_) => "array",
            Value::Object(_) => "struct",
        }
    }

    pub fn as_i32(&self) -> i32 {
        match self {
            Value::I32(v) => *v,
            Value::I64(v) => *v as i32,
            Value::Bool(v) => i32::from(*v),
            Value::F64(v) => *v as i32,
            Value::Char(c) => *c as i32,
            _ => 0,
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            Value::Bool(v) => *v,
            Value::I32(v) => *v != 0,
            _ => false,
        }
    }

    pub fn as_i64(&self) -> i64 {
        match self {
            Value::I64(v) => *v,
            Value::I32(v) => *v as i64,
            Value::F64(v) => *v as i64,
            _ => 0,
        }
    }

    pub fn as_f64(&self) -> f64 {
        match self {
            Value::F64(v) => *v,
            Value::I32(v) => *v as f64,
            Value::I64(v) => *v as f64,
            _ => 0.0,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::I32(v) => write!(f, "{v}"),
            Value::I64(v) => write!(f, "{v}"),
            Value::F64(v) => write!(f, "{v}"),
            Value::Bool(v) => write!(f, "{v}"),
            Value::Str(s) => write!(f, "{s}"),
            Value::Char(c) => write!(f, "{c}"),
            Value::Unit => write!(f, "()"),
            Value::Array(xs) => {
                write!(f, "[")?;
                for (i, x) in xs.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{x}")?;
                }
                write!(f, "]")
            }
            Value::Object(xs) => {
                write!(f, "{{")?;
                for (i, x) in xs.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{x}")?;
                }
                write!(f, "}}")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct VmOptions {
    pub max_steps: u64,
    pub max_call_depth: usize,
    pub trace: bool,
    /// Record (func,pc) → (func,pc) edges for the in-tree greybox fuzzer.
    pub coverage: bool,
    /// Count calls per function for `aether profile`.
    pub profile: bool,
}

impl Default for VmOptions {
    fn default() -> Self {
        VmOptions {
            max_steps: 50_000_000,
            max_call_depth: 10_000,
            trace: false,
            coverage: false,
            profile: false,
        }
    }
}

#[derive(Debug)]
pub enum VmError {
    StepLimit,
    StackOverflow,
    MissingMain,
    Native(String),
    Runtime(String),
    /// Writing to the program's stdout failed because the reader went away
    /// (`EPIPE`, e.g. `aether run f.ae | head -1`). The run stops; the CLI
    /// treats this as a quiet, successful exit.
    OutputClosed,
}

impl fmt::Display for VmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VmError::StepLimit => write!(f, "execution exceeded the instruction step limit"),
            VmError::StackOverflow => write!(f, "call stack overflow"),
            VmError::MissingMain => write!(f, "no `main` function in bytecode module"),
            VmError::Native(s) | VmError::Runtime(s) => write!(f, "{s}"),
            VmError::OutputClosed => write!(f, "stdout was closed"),
        }
    }
}

/// Host implementation of an `extern fn` declared in the script. Arguments
/// were already type-checked by sema against the extern signature.
pub type HostFn = Box<dyn FnMut(&[Value]) -> Result<Value, VmError>>;

/// Outcome of one [`Vm::run_budget`] call.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// `main` returned this value; the VM will not run further.
    Finished(Value),
    /// Budget exhausted or [`Op::Yield`] hit; state kept, call again to resume.
    Yielded,
}

pub struct Vm<'a> {
    module: &'a BytecodeModule,
    opts: VmOptions,
    stdout: Box<dyn Write>,
    steps: u64,
    prev_site: u64,
    edges: HashSet<u64>,
    calls: Vec<u64>,
    host: HashMap<String, HostFn>,
    frames: Vec<Frame>,
    state: State,
    /// `module.strings` as shared values, so `loadstr` never copies text.
    strings: Vec<Str>,
}

struct Frame {
    func: usize,
    pc: usize,
    regs: Vec<Value>,
    ret_reg: Option<u16>,
}

enum State {
    NotStarted,
    Running,
    Finished(Value),
    /// A runtime error was returned; the frame stack is no longer coherent.
    Halted,
}

/// What an instruction asks the dispatch loop to do next.
enum Flow {
    Continue,
    Return(Value),
    Yield,
}

/// Read target for out-of-range registers (never written to). A `const`
/// reference rather than a `static`: `Value` holds `Rc`, so it is not `Sync`.
const UNIT: &Value = &Value::Unit;

impl<'a> Vm<'a> {
    pub fn new(module: &'a BytecodeModule, opts: VmOptions) -> Self {
        Vm {
            module,
            opts,
            stdout: Box::new(io::stdout()),
            steps: 0,
            prev_site: 0,
            edges: HashSet::new(),
            calls: vec![0; module.functions.len()],
            host: HashMap::new(),
            frames: Vec::new(),
            state: State::NotStarted,
            strings: module.strings.iter().map(|s| Str::from(s.as_str())).collect(),
        }
    }

    pub fn with_stdout(mut self, w: Box<dyn Write>) -> Self {
        self.stdout = w;
        self
    }

    /// Bind `extern fn NAME` to a host closure. Re-binding replaces.
    pub fn with_host_fn(mut self, name: &str, f: HostFn) -> Self {
        self.host.insert(name.to_string(), f);
        self
    }

    /// True once `main` has returned (not after an error).
    pub fn is_finished(&self) -> bool {
        matches!(self.state, State::Finished(_))
    }

    /// Run to completion. `Op::Yield` is a no-op here; the total
    /// `opts.max_steps` limit still yields `VmError::StepLimit`.
    pub fn run(&mut self) -> Result<Value, VmError> {
        loop {
            match self.run_budget(u64::MAX)? {
                Step::Finished(v) => return Ok(v),
                Step::Yielded => {}
            }
        }
    }

    /// Execute at most `max_steps` instructions, then return `Yielded` if
    /// the program has not finished. State is kept between calls; the next
    /// call resumes exactly where this one stopped. `Op::Yield` returns
    /// `Yielded` early. Once finished, every call returns the same
    /// `Finished` value; after an error every call returns an error.
    pub fn run_budget(&mut self, max_steps: u64) -> Result<Step, VmError> {
        if let State::Finished(v) = &self.state {
            return Ok(Step::Finished(v.clone()));
        }
        match self.state {
            State::Halted => {
                return Err(VmError::Runtime("vm halted after a previous error".into()))
            }
            State::NotStarted => self.start()?,
            _ => {}
        }
        match self.step_loop(max_steps) {
            Ok(Step::Finished(v)) => {
                self.frames.clear();
                self.state = State::Finished(v.clone());
                Ok(Step::Finished(v))
            }
            Ok(Step::Yielded) => Ok(Step::Yielded),
            Err(e) => {
                self.state = State::Halted;
                Err(e)
            }
        }
    }

    fn start(&mut self) -> Result<(), VmError> {
        let entry = self.module.entry as usize;
        if entry >= self.module.functions.len() {
            return Err(VmError::MissingMain);
        }
        let main = &self.module.functions[entry];
        if main.is_native {
            return Err(VmError::MissingMain);
        }
        let nregs = main.nregs.max(1) as usize;
        if self.opts.profile && entry < self.calls.len() {
            self.calls[entry] += 1;
        }
        self.frames.clear();
        self.frames.push(Frame {
            func: entry,
            pc: 0,
            regs: vec![Value::Unit; nregs],
            ret_reg: None,
        });
        self.state = State::Running;
        Ok(())
    }

    fn step_loop(&mut self, budget: u64) -> Result<Step, VmError> {
        // Copy the shared module reference out so instructions can be
        // borrowed from it while `self.frames` is mutated.
        let module: &'a BytecodeModule = self.module;
        let mut used: u64 = 0;
        loop {
            if used >= budget {
                return Ok(Step::Yielded);
            }
            used += 1;
            if self.frames.len() > self.opts.max_call_depth {
                return Err(VmError::StackOverflow);
            }
            self.steps += 1;
            if self.steps > self.opts.max_steps {
                return Err(VmError::StepLimit);
            }
            let (fi, pc) = {
                let f = self.frames.last().expect("running vm has a frame");
                (f.func, f.pc)
            };
            let func = &module.functions[fi];
            if pc >= func.code.len() {
                if self.frames.len() == 1 {
                    return Ok(Step::Finished(Value::I32(0)));
                }
                let finished = self.frames.pop().expect("frame");
                if let Some(d) = finished.ret_reg {
                    set_reg(Value::Unit, &mut self.frames, d);
                }
                continue;
            }
            let op = &func.code[pc];
            if self.opts.coverage {
                let site = ((fi as u64) << 32) | (pc as u64);
                self.edges.insert(self.prev_site.rotate_left(17) ^ site);
                self.prev_site = site;
            }
            if self.opts.trace {
                let _ = writeln!(self.stdout, "[{fi}:{pc}] {op}");
            }
            self.frames.last_mut().expect("frame").pc += 1;
            match self.exec_op(op)? {
                Flow::Continue => {}
                Flow::Return(v) => return Ok(Step::Finished(v)),
                Flow::Yield => return Ok(Step::Yielded),
            }
        }
    }

    fn exec_op(&mut self, op: &Op) -> Result<Flow, VmError> {
        let module: &'a BytecodeModule = self.module;
        match op {
            Op::LoadImm { dest, imm } => {
                let v = imm_to_value(imm, &self.strings);
                set_reg(v, &mut self.frames, *dest);
            }
            Op::LoadStr { dest, idx } => {
                let s = match self.strings.get(*idx as usize) {
                    Some(s) => s.clone(),
                    None => Str::from(""),
                };
                set_reg(Value::Str(s), &mut self.frames, *dest);
            }
            Op::Move { dest, src } => copy_reg(&mut self.frames, *dest, *src),
            Op::AddI32 { dest, lhs, rhs } => {
                bin_i32(&mut self.frames, *dest, *lhs, *rhs, i32::wrapping_add)
            }
            Op::SubI32 { dest, lhs, rhs } => {
                bin_i32(&mut self.frames, *dest, *lhs, *rhs, i32::wrapping_sub)
            }
            Op::MulI32 { dest, lhs, rhs } => {
                bin_i32(&mut self.frames, *dest, *lhs, *rhs, i32::wrapping_mul)
            }
            Op::DivI32 { dest, lhs, rhs } => {
                let b = read_i32(&self.frames, *rhs);
                if b == 0 {
                    return Err(VmError::Runtime("division by zero".into()));
                }
                let a = read_i32(&self.frames, *lhs);
                set_reg(Value::I32(a.wrapping_div(b)), &mut self.frames, *dest);
            }
            Op::RemI32 { dest, lhs, rhs } => {
                let b = read_i32(&self.frames, *rhs);
                if b == 0 {
                    return Err(VmError::Runtime("division by zero".into()));
                }
                let a = read_i32(&self.frames, *lhs);
                set_reg(Value::I32(a.wrapping_rem(b)), &mut self.frames, *dest);
            }
            Op::NegI32 { dest, src } => {
                let v = read_i32(&self.frames, *src).wrapping_neg();
                set_reg(Value::I32(v), &mut self.frames, *dest);
            }
            Op::AddI64 { dest, lhs, rhs } => {
                bin_i64(&mut self.frames, *dest, *lhs, *rhs, i64::wrapping_add)
            }
            Op::SubI64 { dest, lhs, rhs } => {
                bin_i64(&mut self.frames, *dest, *lhs, *rhs, i64::wrapping_sub)
            }
            Op::MulI64 { dest, lhs, rhs } => {
                bin_i64(&mut self.frames, *dest, *lhs, *rhs, i64::wrapping_mul)
            }
            Op::DivI64 { dest, lhs, rhs } => {
                let b = read_i64(&self.frames, *rhs);
                if b == 0 {
                    return Err(VmError::Runtime("division by zero".into()));
                }
                let a = read_i64(&self.frames, *lhs);
                set_reg(Value::I64(a.wrapping_div(b)), &mut self.frames, *dest);
            }
            Op::RemI64 { dest, lhs, rhs } => {
                let b = read_i64(&self.frames, *rhs);
                if b == 0 {
                    return Err(VmError::Runtime("division by zero".into()));
                }
                let a = read_i64(&self.frames, *lhs);
                set_reg(Value::I64(a.wrapping_rem(b)), &mut self.frames, *dest);
            }
            Op::NegI64 { dest, src } => {
                let v = read_i64(&self.frames, *src).wrapping_neg();
                set_reg(Value::I64(v), &mut self.frames, *dest);
            }
            Op::AddF64 { dest, lhs, rhs } => bin_f64(&mut self.frames, *dest, *lhs, *rhs, |a, b| a + b),
            Op::SubF64 { dest, lhs, rhs } => bin_f64(&mut self.frames, *dest, *lhs, *rhs, |a, b| a - b),
            Op::MulF64 { dest, lhs, rhs } => bin_f64(&mut self.frames, *dest, *lhs, *rhs, |a, b| a * b),
            Op::DivF64 { dest, lhs, rhs } => bin_f64(&mut self.frames, *dest, *lhs, *rhs, |a, b| a / b),
            Op::NegF64 { dest, src } => {
                let v = -read_f64(&self.frames, *src);
                set_reg(Value::F64(v), &mut self.frames, *dest);
            }
            Op::CmpEqI32 { dest, lhs, rhs } => cmp_i32(&mut self.frames, *dest, *lhs, *rhs, |a, b| a == b),
            Op::CmpNeI32 { dest, lhs, rhs } => cmp_i32(&mut self.frames, *dest, *lhs, *rhs, |a, b| a != b),
            Op::CmpLtI32 { dest, lhs, rhs } => cmp_i32(&mut self.frames, *dest, *lhs, *rhs, |a, b| a < b),
            Op::CmpLeI32 { dest, lhs, rhs } => cmp_i32(&mut self.frames, *dest, *lhs, *rhs, |a, b| a <= b),
            Op::CmpGtI32 { dest, lhs, rhs } => cmp_i32(&mut self.frames, *dest, *lhs, *rhs, |a, b| a > b),
            Op::CmpGeI32 { dest, lhs, rhs } => cmp_i32(&mut self.frames, *dest, *lhs, *rhs, |a, b| a >= b),
            Op::CmpEqI64 { dest, lhs, rhs } => {
                let r = read_i64(&self.frames, *lhs) == read_i64(&self.frames, *rhs);
                set_reg(Value::Bool(r), &mut self.frames, *dest);
            }
            Op::CmpLtI64 { dest, lhs, rhs } => {
                let r = read_i64(&self.frames, *lhs) < read_i64(&self.frames, *rhs);
                set_reg(Value::Bool(r), &mut self.frames, *dest);
            }
            Op::CmpEqF64 { dest, lhs, rhs } => {
                let r = read_f64(&self.frames, *lhs) == read_f64(&self.frames, *rhs);
                set_reg(Value::Bool(r), &mut self.frames, *dest);
            }
            Op::CmpLtF64 { dest, lhs, rhs } => {
                let r = read_f64(&self.frames, *lhs) < read_f64(&self.frames, *rhs);
                set_reg(Value::Bool(r), &mut self.frames, *dest);
            }
            Op::CmpEqBool { dest, lhs, rhs } => {
                let r = read_bool(&self.frames, *lhs) == read_bool(&self.frames, *rhs);
                set_reg(Value::Bool(r), &mut self.frames, *dest);
            }
            Op::Cmp { op: cmp, dest, lhs, rhs } => {
                let r = cmp_values(*cmp, reg(&self.frames, *lhs), reg(&self.frames, *rhs))?;
                set_reg(Value::Bool(r), &mut self.frames, *dest);
            }
            Op::AndBool { dest, lhs, rhs } => {
                let r = read_bool(&self.frames, *lhs) && read_bool(&self.frames, *rhs);
                set_reg(Value::Bool(r), &mut self.frames, *dest);
            }
            Op::OrBool { dest, lhs, rhs } => {
                let r = read_bool(&self.frames, *lhs) || read_bool(&self.frames, *rhs);
                set_reg(Value::Bool(r), &mut self.frames, *dest);
            }
            Op::NotBool { dest, src } => {
                let r = !read_bool(&self.frames, *src);
                set_reg(Value::Bool(r), &mut self.frames, *dest);
            }
            Op::BitAnd { dest, lhs, rhs } => bit_op(&mut self.frames, *dest, *lhs, *rhs, |a, b| a & b, |a, b| a & b),
            Op::BitOr { dest, lhs, rhs } => bit_op(&mut self.frames, *dest, *lhs, *rhs, |a, b| a | b, |a, b| a | b),
            Op::BitXor { dest, lhs, rhs } => bit_op(&mut self.frames, *dest, *lhs, *rhs, |a, b| a ^ b, |a, b| a ^ b),
            Op::Shl { dest, lhs, rhs } => bit_op(
                &mut self.frames,
                *dest,
                *lhs,
                *rhs,
                |a, b| a.wrapping_shl((b & 31) as u32),
                |a, b| a.wrapping_shl((b & 63) as u32),
            ),
            Op::Shr { dest, lhs, rhs } => bit_op(
                &mut self.frames,
                *dest,
                *lhs,
                *rhs,
                |a, b| a.wrapping_shr((b & 31) as u32),
                |a, b| a.wrapping_shr((b & 63) as u32),
            ),
            Op::NotInt { dest, src } => {
                let v = match reg(&self.frames, *src) {
                    Value::I64(x) => Value::I64(!x),
                    other => Value::I32(!other.as_i32()),
                };
                set_reg(v, &mut self.frames, *dest);
            }
            Op::Jump { target } => self.frames.last_mut().expect("frame").pc = *target as usize,
            Op::JumpIf { cond, target } => {
                if reg(&self.frames, *cond).as_bool() {
                    self.frames.last_mut().expect("frame").pc = *target as usize;
                }
            }
            Op::JumpIfNot { cond, target } => {
                if !reg(&self.frames, *cond).as_bool() {
                    self.frames.last_mut().expect("frame").pc = *target as usize;
                }
            }
            Op::Call { func, dest, args } => {
                let callee = *func as usize;
                let Some(callee_fn) = module.functions.get(callee) else {
                    return Err(VmError::Runtime(format!("invalid function index {func}")));
                };
                let argv: Vec<Value> = args.iter().map(|r| reg(&self.frames, *r).clone()).collect();
                if callee_fn.is_native {
                    let result = match callee_fn.native_id {
                        Some(nid) => self.call_native(nid, &argv)?,
                        None => self.call_host(callee_fn, &argv)?,
                    };
                    if let Some(d) = dest {
                        set_reg(result, &mut self.frames, *d);
                    }
                } else {
                    let nregs = (callee_fn.nregs as usize).max(args.len());
                    let mut regs = vec![Value::Unit; nregs.max(1)];
                    for (i, v) in argv.into_iter().enumerate() {
                        if i < regs.len() {
                            regs[i] = v;
                        }
                    }
                    if self.opts.profile && callee < self.calls.len() {
                        self.calls[callee] += 1;
                    }
                    self.frames.push(Frame {
                        func: callee,
                        pc: 0,
                        regs,
                        ret_reg: *dest,
                    });
                }
            }
            Op::CallNative { id, dest, args } => {
                let argv: Vec<Value> = args.iter().map(|r| reg(&self.frames, *r).clone()).collect();
                let result = self.call_native(*id, &argv)?;
                if let Some(d) = dest {
                    set_reg(result, &mut self.frames, *d);
                }
            }
            Op::Ret { src } => {
                // The returning frame is discarded, so the value can be
                // moved out instead of cloned (same observable result).
                let v = self
                    .frames
                    .last_mut()
                    .and_then(|f| f.regs.get_mut(*src as usize))
                    .map(|slot| std::mem::replace(slot, Value::Unit))
                    .unwrap_or(Value::Unit);
                if self.frames.len() == 1 {
                    return Ok(Flow::Return(v));
                }
                let finished = self.frames.pop().expect("frame");
                if let Some(d) = finished.ret_reg {
                    set_reg(v, &mut self.frames, d);
                }
            }
            Op::RetVoid => {
                if self.frames.len() == 1 {
                    return Ok(Flow::Return(Value::Unit));
                }
                let finished = self.frames.pop().expect("frame");
                if let Some(d) = finished.ret_reg {
                    set_reg(Value::Unit, &mut self.frames, d);
                }
            }
            Op::CastI32ToI64 { dest, src } => {
                let v = read_i32(&self.frames, *src) as i64;
                set_reg(Value::I64(v), &mut self.frames, *dest);
            }
            Op::CastI64ToI32 { dest, src } => {
                let v = read_i64(&self.frames, *src) as i32;
                set_reg(Value::I32(v), &mut self.frames, *dest);
            }
            Op::CastI32ToF64 { dest, src } => {
                let v = read_i32(&self.frames, *src) as f64;
                set_reg(Value::F64(v), &mut self.frames, *dest);
            }
            Op::CastF64ToI32 { dest, src } => {
                let v = read_f64(&self.frames, *src) as i32;
                set_reg(Value::I32(v), &mut self.frames, *dest);
            }
            Op::CastBoolToI32 { dest, src } => {
                let v = i32::from(read_bool(&self.frames, *src));
                set_reg(Value::I32(v), &mut self.frames, *dest);
            }
            Op::CastI64ToF64 { dest, src } => {
                let v = read_i64(&self.frames, *src) as f64;
                set_reg(Value::F64(v), &mut self.frames, *dest);
            }
            Op::CastF64ToI64 { dest, src } => {
                let v = read_f64(&self.frames, *src) as i64;
                set_reg(Value::I64(v), &mut self.frames, *dest);
            }
            Op::CastBoolToI64 { dest, src } => {
                let v = i64::from(read_bool(&self.frames, *src));
                set_reg(Value::I64(v), &mut self.frames, *dest);
            }
            Op::CastCharToI32 { dest, src } => {
                // `as_i32` already maps `Char(c)` to `c as i32`.
                let v = read_i32(&self.frames, *src);
                set_reg(Value::I32(v), &mut self.frames, *dest);
            }
            Op::CastI32ToChar { dest, src } => {
                let c = u32::try_from(read_i32(&self.frames, *src))
                    .ok()
                    .and_then(char::from_u32)
                    .unwrap_or('\u{FFFD}');
                set_reg(Value::Char(c), &mut self.frames, *dest);
            }
            Op::AllocArr { dest, len } => {
                if *len as usize > MAX_ARRAY_LEN {
                    return Err(array_too_long(*len));
                }
                set_reg(Value::array(vec![Value::I32(0); *len as usize]), &mut self.frames, *dest);
            }
            Op::LoadIdx { dest, base, index } => {
                let idx = match reg(&self.frames, *index) {
                    Value::I32(i) => *i,
                    other => return Err(index_err(other)),
                };
                // Only the element is cloned, never the container.
                let v = match reg(&self.frames, *base) {
                    Value::Array(xs) => {
                        if idx < 0 || idx as usize >= xs.len() {
                            return Err(VmError::Runtime(format!("array index {idx} out of bounds")));
                        }
                        xs[idx as usize].clone()
                    }
                    Value::Str(s) => {
                        let c = if idx < 0 { None } else { s.char_at(idx as usize) };
                        match c {
                            Some(c) => Value::Char(c),
                            None => return Err(VmError::Runtime("string index out of bounds".into())),
                        }
                    }
                    other => return Err(type_err("index", other)),
                };
                set_reg(v, &mut self.frames, *dest);
            }
            Op::StoreIdx { base, index, value } => {
                let idx = match reg(&self.frames, *index) {
                    Value::I32(i) => *i,
                    other => return Err(index_err(other)),
                };
                let val = reg(&self.frames, *value).clone();
                let frame = self.frames.last_mut().expect("frame");
                match frame.regs.get_mut(*base as usize) {
                    Some(Value::Array(xs)) => {
                        if idx < 0 || idx as usize >= xs.len() {
                            return Err(VmError::Runtime(format!("array index {idx} out of bounds")));
                        }
                        // Copy-on-write: only copies the buffer if another
                        // register/value still shares it.
                        Rc::make_mut(xs)[idx as usize] = val;
                    }
                    Some(other) => return Err(type_err("store into", other)),
                    None => return Err(VmError::Runtime("store into a missing register".into())),
                }
            }
            Op::AllocObj { dest, fields } => {
                set_reg(Value::object(vec![Value::Unit; *fields as usize]), &mut self.frames, *dest);
            }
            Op::LoadField { dest, base, field } => {
                let v = match reg(&self.frames, *base) {
                    Value::Object(xs) => match xs.get(*field as usize) {
                        Some(v) => v.clone(),
                        None => return Err(field_err(*field, Some(xs.len()), "read", "object")),
                    },
                    other => return Err(field_err(*field, None, "read", other.type_name())),
                };
                set_reg(v, &mut self.frames, *dest);
            }
            Op::StoreField { base, field, value } => {
                let val = reg(&self.frames, *value).clone();
                let frame = self.frames.last_mut().expect("frame");
                match frame.regs.get_mut(*base as usize) {
                    Some(Value::Object(xs)) => {
                        if (*field as usize) >= xs.len() {
                            return Err(field_err(*field, Some(xs.len()), "write", "object"));
                        }
                        Rc::make_mut(xs)[*field as usize] = val;
                    }
                    Some(other) => return Err(field_err(*field, None, "write", other.type_name())),
                    None => return Err(VmError::Runtime("store into a missing register".into())),
                }
            }
            Op::Concat { dest, lhs, rhs } => {
                if dest == lhs && lhs != rhs && self.concat_in_place(*lhs, *rhs)? {
                    return Ok(Flow::Continue);
                }
                let s = match (reg(&self.frames, *lhs), reg(&self.frames, *rhs)) {
                    (Value::Str(a), Value::Str(b)) => {
                        let n = a.len().saturating_add(b.len());
                        if n > MAX_STRING_BYTES {
                            return Err(string_too_long(n));
                        }
                        let mut s = String::with_capacity(n);
                        s.push_str(a);
                        s.push_str(b);
                        // ASCII-ness is known from the operands: no rescan.
                        let ascii = a.is_ascii() && b.is_ascii();
                        Str(Rc::new(StrBuf { text: s, ascii, offsets: OnceCell::new() }))
                    }
                    (a, b) => return Err(concat_err(a, b)),
                };
                set_reg(Value::Str(s), &mut self.frames, *dest);
            }
            Op::Yield => return Ok(Flow::Yield),
            Op::Nop => {}
        }
        Ok(Flow::Continue)
    }

    /// `r = r + t` on strings: appends to `r`'s text in place when no other
    /// value shares it (amortised O(len t)). `Ok(false)` means the general
    /// copying path must run (text shared, or an operand is not a string).
    fn concat_in_place(&mut self, lhs: u16, rhs: u16) -> Result<bool, VmError> {
        let Some(frame) = self.frames.last_mut() else { return Ok(false) };
        let (l, r) = (lhs as usize, rhs as usize);
        if l >= frame.regs.len() || r >= frame.regs.len() {
            return Ok(false);
        }
        let mut taken = std::mem::replace(&mut frame.regs[l], Value::Unit);
        let done = match (&mut taken, &frame.regs[r]) {
            (Value::Str(a), Value::Str(b)) => {
                let n = a.len().saturating_add(b.len());
                if n > MAX_STRING_BYTES {
                    frame.regs[l] = taken;
                    return Err(string_too_long(n));
                }
                a.try_push_str(b)
            }
            _ => false,
        };
        frame.regs[l] = taken;
        Ok(done)
    }

    /// Runs the host closure bound to an `extern fn` and checks what it
    /// returned against the declared return type.
    #[inline(never)]
    fn call_host(&mut self, callee_fn: &BcFunction, argv: &[Value]) -> Result<Value, VmError> {
        let Some(f) = self.host.get_mut(callee_fn.name.as_str()) else {
            return Err(VmError::Native(format!(
                "extern function `{}` has no implementation in the VM",
                callee_fn.name
            )));
        };
        let v = f(argv)?;
        match &callee_fn.ret_ty {
            // unit externs: whatever the host returns is dropped
            Some(Type::Unit) => Ok(Value::Unit),
            Some(t) if !value_has_type(&v, t) => Err(VmError::Native(format!(
                "extern function `{}` returned {} but is declared to return `{t}`",
                callee_fn.name,
                describe(&v)
            ))),
            _ => Ok(v),
        }
    }

    #[inline(never)]
    fn call_native(&mut self, id: u16, args: &[Value]) -> Result<Value, VmError> {
        let io_err = |e: io::Error| {
            if e.kind() == io::ErrorKind::BrokenPipe {
                VmError::OutputClosed
            } else {
                VmError::Native(e.to_string())
            }
        };
        match id {
            0 => {
                let s = arg_str(id, args, 0)?;
                write!(self.stdout, "{s}").map_err(io_err)?;
                Ok(Value::Unit)
            }
            1 => {
                let s = arg_str(id, args, 0)?;
                writeln!(self.stdout, "{s}").map_err(io_err)?;
                Ok(Value::Unit)
            }
            2 => {
                let v = arg_i32(id, args, 0)?;
                writeln!(self.stdout, "{v}").map_err(io_err)?;
                Ok(Value::Unit)
            }
            3 => {
                let v = arg_i64(id, args, 0)?;
                writeln!(self.stdout, "{v}").map_err(io_err)?;
                Ok(Value::Unit)
            }
            4 => {
                let v = arg_f64(id, args, 0)?;
                writeln!(self.stdout, "{v}").map_err(io_err)?;
                Ok(Value::Unit)
            }
            5 => {
                let v = arg_bool(id, args, 0)?;
                writeln!(self.stdout, "{v}").map_err(io_err)?;
                Ok(Value::Unit)
            }
            6 => match args.first() {
                Some(Value::Str(s)) => Ok(Value::I32(i32::try_from(s.char_len()).unwrap_or(i32::MAX))),
                Some(Value::Array(xs)) => Ok(Value::I32(i32::try_from(xs.len()).unwrap_or(i32::MAX))),
                other => Err(bad_arg(id, 0, "string or array", other)),
            },
            7 => {
                if !arg_bool(id, args, 0)? {
                    return Err(VmError::Native("assertion failed".into()));
                }
                Ok(Value::Unit)
            }
            8 => {
                let c = arg_char(id, args, 0)?;
                writeln!(self.stdout, "{c}").map_err(io_err)?;
                Ok(Value::Unit)
            }
            9 => Ok(Value::str(arg_i32(id, args, 0)?.to_string())),
            10 => Ok(Value::str(arg_i64(id, args, 0)?.to_string())),
            11 => Ok(Value::str(arg_f64(id, args, 0)?.to_string())),
            12 => Ok(Value::str(arg_char(id, args, 0)?.to_string())),
            13 => Ok(Value::I32(arg_i32(id, args, 0)?.wrapping_abs())),
            14 => Ok(Value::I32(arg_i32(id, args, 0)?.min(arg_i32(id, args, 1)?))),
            15 => Ok(Value::I32(arg_i32(id, args, 0)?.max(arg_i32(id, args, 1)?))),
            16 => {
                let (x, lo, hi) = (arg_i32(id, args, 0)?, arg_i32(id, args, 1)?, arg_i32(id, args, 2)?);
                Ok(Value::I32(lo.max(hi.min(x))))
            }
            17 => Ok(Value::F64(arg_f64(id, args, 0)?.sqrt())),
            18 => Ok(Value::F64(arg_f64(id, args, 0)?.floor())),
            19 => Ok(Value::F64(arg_f64(id, args, 0)?.ceil())),
            20 => {
                let (b, e) = (arg_i32(id, args, 0)?, arg_i32(id, args, 1)?);
                Ok(Value::I32(if e < 0 { 0 } else { b.wrapping_pow(e as u32) }))
            }
            _ => Err(VmError::Native(format!("unknown native #{id}"))),
        }
    }

    pub fn steps(&self) -> u64 {
        self.steps
    }

    pub fn coverage(&self) -> &HashSet<u64> {
        &self.edges
    }

    pub fn call_counts(&self) -> &[u64] {
        &self.calls
    }
}

/// FNV-1a over return value and captured stdout. Same source + same seed
/// of the compiler must yield the same digest (catalog item 5.10 replay).
pub fn digest(value: &Value, stdout: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    let bytes = format!("{value}\n{stdout}").into_bytes();
    for b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[derive(Debug, Clone)]
pub struct ProfileReport {
    pub steps: u64,
    pub digest: u64,
    pub functions: Vec<(String, u64)>,
}

impl ProfileReport {
    pub fn render(&self) -> String {
        let mut s = format!("steps={} digest={:#x}\n", self.steps, self.digest);
        let mut rows = self.functions.clone();
        rows.sort_by(|a, b| b.1.cmp(&a.1));
        for (name, n) in rows {
            if n > 0 {
                s.push_str(&format!("  {n:>8}  {name}\n"));
            }
        }
        s
    }
}

/// Borrow register `r` of the top frame; unset registers read as `Unit`.
fn reg(frames: &[Frame], r: u16) -> &Value {
    frames
        .last()
        .and_then(|f| f.regs.get(r as usize))
        .unwrap_or(UNIT)
}

fn set_reg(v: Value, frames: &mut [Frame], r: u16) {
    if let Some(f) = frames.last_mut() {
        let i = r as usize;
        if i >= f.regs.len() {
            f.regs.resize(i + 1, Value::Unit);
        }
        f.regs[i] = v;
    }
}

fn copy_reg(frames: &mut [Frame], dest: u16, src: u16) {
    let v = reg(frames, src).clone();
    set_reg(v, frames, dest);
}

fn read_i32(frames: &[Frame], r: u16) -> i32 { reg(frames, r).as_i32() }

/// Largest string the VM builds by concatenation (256 MiB). Without a bound,
/// `s = s + s` in a loop aborts the whole process on allocation failure.
pub const MAX_STRING_BYTES: usize = 1 << 28;

#[cold]
#[inline(never)]
fn index_err(v: &Value) -> VmError {
    VmError::Runtime(format!("index must be i32, got {}", v.type_name()))
}

// Error constructors live out of line so the dispatch loop stays small.

#[cold]
#[inline(never)]
fn field_err(field: u16, len: Option<usize>, verb: &str, ty: &str) -> VmError {
    VmError::Runtime(match len {
        Some(n) => format!("cannot {verb} field {field}: the object has {n} fields"),
        None => format!("cannot {verb} field {field} of a value of type {ty}"),
    })
}

#[cold]
#[inline(never)]
fn type_err(what: &str, v: &Value) -> VmError {
    VmError::Runtime(format!("cannot {what} a value of type {}", v.type_name()))
}

#[cold]
#[inline(never)]
fn concat_err(a: &Value, b: &Value) -> VmError {
    VmError::Runtime(format!("cannot concatenate {} with {}", a.type_name(), b.type_name()))
}

#[cold]
#[inline(never)]
fn string_too_long(n: usize) -> VmError {
    VmError::Runtime(format!("string of {n} bytes exceeds the limit of {MAX_STRING_BYTES}"))
}

#[cold]
#[inline(never)]
fn array_too_long(n: u32) -> VmError {
    VmError::Runtime(format!("array of {n} elements exceeds the limit of {MAX_ARRAY_LEN}"))
}

fn bad_arg(id: u16, i: usize, want: &str, got: Option<&Value>) -> VmError {
    let name = crate::runtime::NATIVES.get(id as usize).copied().unwrap_or("?");
    let got = got.map_or("nothing", Value::type_name);
    VmError::Native(format!("`{name}` expects {want} for argument {}, got {got}", i + 1))
}

fn arg_i32(id: u16, args: &[Value], i: usize) -> Result<i32, VmError> {
    match args.get(i) {
        Some(Value::I32(v)) => Ok(*v),
        other => Err(bad_arg(id, i, "i32", other)),
    }
}

fn arg_i64(id: u16, args: &[Value], i: usize) -> Result<i64, VmError> {
    match args.get(i) {
        Some(Value::I64(v)) => Ok(*v),
        other => Err(bad_arg(id, i, "i64", other)),
    }
}

fn arg_f64(id: u16, args: &[Value], i: usize) -> Result<f64, VmError> {
    match args.get(i) {
        Some(Value::F64(v)) => Ok(*v),
        other => Err(bad_arg(id, i, "f64", other)),
    }
}

fn arg_bool(id: u16, args: &[Value], i: usize) -> Result<bool, VmError> {
    match args.get(i) {
        Some(Value::Bool(v)) => Ok(*v),
        other => Err(bad_arg(id, i, "bool", other)),
    }
}

fn arg_char(id: u16, args: &[Value], i: usize) -> Result<char, VmError> {
    match args.get(i) {
        Some(Value::Char(v)) => Ok(*v),
        other => Err(bad_arg(id, i, "char", other)),
    }
}

fn arg_str(id: u16, args: &[Value], i: usize) -> Result<&str, VmError> {
    match args.get(i) {
        Some(Value::Str(v)) => Ok(v),
        other => Err(bad_arg(id, i, "string", other)),
    }
}

/// Short description of a value for error messages (never prints a whole array).
fn describe(v: &Value) -> String {
    match v {
        Value::Array(xs) => format!("an array of {} elements", xs.len()),
        Value::Object(xs) => format!("an object of {} fields", xs.len()),
        Value::Unit => "unit".to_string(),
        other => format!("{} `{other}`", other.type_name()),
    }
}

/// Does `v` have the shape of source type `t`? Used on values a host closure
/// hands back for an `extern fn`, which the type checker never saw.
fn value_has_type(v: &Value, t: &Type) -> bool {
    match (t, v) {
        (Type::Unit, Value::Unit)
        | (Type::Bool, Value::Bool(_))
        | (Type::I32, Value::I32(_))
        | (Type::I64, Value::I64(_))
        | (Type::F64, Value::F64(_))
        | (Type::String, Value::Str(_))
        | (Type::Char, Value::Char(_)) => true,
        (Type::Array { elem, len }, Value::Array(xs)) => {
            i64::try_from(xs.len()).map_or(false, |n| n == *len) && xs.iter().all(|x| value_has_type(x, elem))
        }
        (Type::Struct { .. } | Type::Tuple(_), Value::Object(xs)) => match t.layout_fields() {
            Some(fields) => fields.len() == xs.len() && fields.iter().zip(xs.iter()).all(|(ft, x)| value_has_type(x, ft)),
            None => false,
        },
        (Type::Enum { variants, .. }, Value::Object(xs)) => {
            let Some(layout) = t.layout_fields() else { return false };
            if xs.len() != layout.len() {
                return false;
            }
            let Value::I32(tag) = xs[0] else { return false };
            let Some((_, payload)) = usize::try_from(tag).ok().and_then(|i| variants.get(i)) else {
                return false;
            };
            payload.iter().enumerate().all(|(i, pt)| value_has_type(&xs[i + 1], pt))
        }
        (Type::Error | Type::Fn { .. }, _) => true,
        _ => false,
    }
}

/// Integer bit operation dispatched on the left operand's width.
fn bit_op(frames: &mut [Frame], dest: u16, lhs: u16, rhs: u16, f32: fn(i32, i32) -> i32, f64: fn(i64, i64) -> i64) {
    let v = match reg(frames, lhs) {
        Value::I64(a) => Value::I64(f64(*a, read_i64(frames, rhs))),
        other => Value::I32(f32(other.as_i32(), read_i32(frames, rhs))),
    };
    set_reg(v, frames, dest);
}
fn read_i64(frames: &[Frame], r: u16) -> i64 { reg(frames, r).as_i64() }
fn read_f64(frames: &[Frame], r: u16) -> f64 { reg(frames, r).as_f64() }
fn read_bool(frames: &[Frame], r: u16) -> bool { reg(frames, r).as_bool() }

fn bin_i32(frames: &mut [Frame], dest: u16, lhs: u16, rhs: u16, f: fn(i32, i32) -> i32) {
    let v = f(read_i32(frames, lhs), read_i32(frames, rhs));
    set_reg(Value::I32(v), frames, dest);
}

fn bin_i64(frames: &mut [Frame], dest: u16, lhs: u16, rhs: u16, f: fn(i64, i64) -> i64) {
    let v = f(read_i64(frames, lhs), read_i64(frames, rhs));
    set_reg(Value::I64(v), frames, dest);
}

fn bin_f64(frames: &mut [Frame], dest: u16, lhs: u16, rhs: u16, f: fn(f64, f64) -> f64) {
    let v = f(read_f64(frames, lhs), read_f64(frames, rhs));
    set_reg(Value::F64(v), frames, dest);
}

fn cmp_i32(frames: &mut [Frame], dest: u16, lhs: u16, rhs: u16, f: fn(i32, i32) -> bool) {
    let v = f(read_i32(frames, lhs), read_i32(frames, rhs));
    set_reg(Value::Bool(v), frames, dest);
}

/// Generic comparison for [`Op::Cmp`]: both operands must carry the same
/// runtime tag; ordering is defined for numbers, bools, chars and strings.
fn cmp_values(op: CmpOp, a: &Value, b: &Value) -> Result<bool, VmError> {
    fn by_ord(op: CmpOp, o: std::cmp::Ordering) -> bool {
        match op {
            CmpOp::Eq => o.is_eq(),
            CmpOp::Ne => o.is_ne(),
            CmpOp::Lt => o.is_lt(),
            CmpOp::Le => o.is_le(),
            CmpOp::Gt => o.is_gt(),
            CmpOp::Ge => o.is_ge(),
        }
    }
    Ok(match (a, b) {
        (Value::I32(x), Value::I32(y)) => by_ord(op, x.cmp(y)),
        (Value::I64(x), Value::I64(y)) => by_ord(op, x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => by_ord(op, x.cmp(y)),
        (Value::Char(x), Value::Char(y)) => by_ord(op, x.cmp(y)),
        (Value::Str(x), Value::Str(y)) => by_ord(op, x.cmp(y)),
        // IEEE semantics: NaN is unordered, so only `!=` holds.
        (Value::F64(x), Value::F64(y)) => match op {
            CmpOp::Eq => x == y,
            CmpOp::Ne => x != y,
            CmpOp::Lt => x < y,
            CmpOp::Le => x <= y,
            CmpOp::Gt => x > y,
            CmpOp::Ge => x >= y,
        },
        (Value::Unit, Value::Unit) => matches!(op, CmpOp::Eq | CmpOp::Le | CmpOp::Ge),
        // Aggregates compare element by element (IEEE for floats, like the
        // lowering of tuple/struct `==`); only `==` and `!=` are defined.
        (Value::Array(_), Value::Array(_)) | (Value::Object(_), Value::Object(_)) => match op {
            CmpOp::Eq => a == b,
            CmpOp::Ne => a != b,
            _ => {
                return Err(VmError::Runtime(format!(
                    "cannot order two values of type {}",
                    a.type_name()
                )))
            }
        },
        _ => {
            return Err(VmError::Runtime(format!(
                "cannot compare {} with {}",
                a.type_name(),
                b.type_name()
            )))
        }
    })
}

fn imm_to_value(imm: &Immediate, strings: &[Str]) -> Value {
    match imm {
        Immediate::I32(v) => Value::I32(*v),
        Immediate::I64(v) => Value::I64(*v),
        Immediate::F64(bits) => Value::F64(f64::from_bits(*bits)),
        Immediate::Bool(v) => Value::Bool(*v),
        Immediate::Str(i) => Value::Str(strings.get(*i as usize).cloned().unwrap_or_else(|| Str::from(""))),
        Immediate::Char(c) => Value::Char(char::from_u32(*c).unwrap_or('\0')),
        Immediate::Unit => Value::Unit,
    }
}

pub fn execute(module: &BytecodeModule) -> Result<Value, VmError> {
    Vm::new(module, VmOptions::default()).run()
}

struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Run `main` with stdout captured, custom options and host-bound externs.
/// The partial stdout and step count are returned even when the VM fails,
/// so callers can show what the program printed before the error.
pub fn execute_captured_with(
    module: &BytecodeModule,
    opts: VmOptions,
    host: Vec<(String, HostFn)>,
) -> (Result<Value, VmError>, String, u64) {
    let slot = Arc::new(Mutex::new(Vec::new()));
    let mut vm = Vm::new(module, opts).with_stdout(Box::new(Sink(slot.clone())));
    for (name, f) in host {
        vm = vm.with_host_fn(&name, f);
    }
    let result = vm.run();
    let out = String::from_utf8_lossy(&slot.lock().unwrap()).into_owned();
    (result, out, vm.steps())
}

/// Run `main` with stdout captured. Unlike [`execute_captured`] the
/// partial stdout and step count are returned even when the VM fails.
pub fn run_captured(module: &BytecodeModule) -> (Result<Value, VmError>, String, u64) {
    execute_captured_with(module, VmOptions::default(), Vec::new())
}

pub fn execute_captured(module: &BytecodeModule) -> Result<(Value, String, u64), VmError> {
    let (result, out, steps) = run_captured(module);
    result.map(|val| (val, out, steps))
}

/// Same as [`execute_captured`] but fills an edge-coverage set for greybox.
pub fn execute_with_coverage(
    module: &BytecodeModule,
) -> Result<(Value, String, u64, HashSet<u64>), VmError> {
    let slot = Arc::new(Mutex::new(Vec::new()));
    let opts = VmOptions {
        coverage: true,
        ..VmOptions::default()
    };
    let mut vm = Vm::new(module, opts).with_stdout(Box::new(Sink(slot.clone())));
    let val = vm.run()?;
    let out = String::from_utf8_lossy(&slot.lock().unwrap()).into_owned();
    Ok((val, out, vm.steps(), vm.edges))
}

pub fn execute_profiled(module: &BytecodeModule) -> Result<(Value, String, ProfileReport), VmError> {
    let (result, out, report) = execute_profiled_with(module, VmOptions::default());
    result.map(|val| (val, out, report))
}

/// One profiled run with explicit limits. Unlike [`execute_profiled`] the
/// partial stdout and the report (steps and call counts so far) are returned
/// even when the program fails, so the CLI never re-runs it to recover them.
/// On failure the digest covers `()` and the partial stdout.
pub fn execute_profiled_with(
    module: &BytecodeModule,
    opts: VmOptions,
) -> (Result<Value, VmError>, String, ProfileReport) {
    let slot = Arc::new(Mutex::new(Vec::new()));
    let opts = VmOptions { profile: true, ..opts };
    let mut vm = Vm::new(module, opts).with_stdout(Box::new(Sink(slot.clone())));
    let result = vm.run();
    let out = String::from_utf8_lossy(&slot.lock().unwrap()).into_owned();
    let functions = module
        .functions
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.clone(), vm.calls.get(i).copied().unwrap_or(0)))
        .collect();
    let report = ProfileReport {
        steps: vm.steps(),
        digest: digest(result.as_ref().unwrap_or(&Value::Unit), &out),
        functions,
    };
    (result, out, report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::assemble;
    use crate::backend::bytecode::BcFunction;
    use crate::ir::emit_ir;
    use crate::lexer::tokenize;
    use crate::opt::optimize;
    use crate::parser::parse;
    use crate::sema::analyze;
    use crate::span::FileId;

    fn run_src(src: &str) -> (Value, String) { run_src_at(src, 2) }

    fn run_src_at(src: &str, level: u8) -> (Value, String) {
        let (toks, d1) = tokenize(FileId(0), src);
        assert!(!d1.has_errors());
        let (prog, d2) = parse(toks);
        assert!(!d2.has_errors());
        let (hir, d3) = analyze(&prog);
        assert!(!d3.has_errors());
        let ir = emit_ir(&hir.unwrap());
        let (ir, _) = optimize(ir, level);
        let bc = assemble(&ir).expect("assemble");
        let (v, out, _) = execute_captured(&bc).expect("vm");
        (v, out)
    }

    #[test]
    fn runs_arithmetic() {
        let (v, _) = run_src("fn main() -> i32 { return 10 + 32; }");
        assert_eq!(v, Value::I32(42));
    }

    #[test]
    fn runs_fib() {
        let src = r#"
            fn fib(n: i32) -> i32 {
                if n < 2 { return n; }
                return fib(n - 1) + fib(n - 2);
            }
            fn main() -> i32 { return fib(10); }
        "#;
        let (v, _) = run_src(src);
        assert_eq!(v, Value::I32(55));
    }

    #[test]
    fn runs_loop_and_print() {
        let src = r#"
            fn main() -> i32 {
                let mut s = 0;
                for i in 1..11 {
                    s = s + i;
                }
                print_i32(s);
                return s;
            }
        "#;
        let (v, out) = run_src(src);
        assert_eq!(v, Value::I32(55));
        assert!(out.contains("55"));
    }

    #[test]
    fn profile_counts_fib_calls() {
        let src = r#"
            fn fib(n: i32) -> i32 {
                if n < 2 { return n; }
                return fib(n - 1) + fib(n - 2);
            }
            fn main() -> i32 { return fib(6); }
        "#;
        let (toks, _) = tokenize(FileId(0), src);
        let (prog, _) = parse(toks);
        let (hir, _) = analyze(&prog);
        let ir = emit_ir(&hir.unwrap());
        let bc = assemble(&ir).expect("assemble");
        let (v, _, report) = execute_profiled(&bc).expect("vm");
        assert_eq!(v, Value::I32(8));
        let fib = report
            .functions
            .iter()
            .find(|(n, _)| n == "fib")
            .map(|(_, c)| *c)
            .unwrap_or(0);
        assert!(fib >= 1, "{}", report.render());
        assert_ne!(report.digest, 0);
    }

    fn bytecode_of(src: &str) -> BytecodeModule {
        let (toks, _) = tokenize(FileId(0), src);
        let (prog, _) = parse(toks);
        let (hir, d) = analyze(&prog);
        assert!(!d.has_errors(), "{d:?}");
        assemble(&emit_ir(&hir.unwrap())).expect("assemble")
    }

    #[test]
    fn generic_cmp_casts_and_i64_ops() {
        let src = r#"
            fn main() -> i32 {
                let a: i64 = 5000000000;
                print_i64(-a);
                let c: i64 = 7;
                let d: i64 = 3;
                print_i64(c % d);
                print_bool(c != d);
                print_bool(c >= d);
                let s = "abc";
                print_bool(s == "abc");
                print_bool('a' < 'b');
                print_bool(true != false);
                print_bool(1.5 >= 2.0);
                print_i32(len("ação"));
                print_f64(c as f64);
                print_i32('A' as i32);
                print_i32((65 as char) as i32);
                print_i64(true as i64);
                return 0;
            }
        "#;
        let (_, out) = run_src(src);
        assert_eq!(
            out,
            "-5000000000\n1\ntrue\ntrue\ntrue\ntrue\ntrue\nfalse\n4\n7\n65\n65\n1\n"
        );
    }

    #[test]
    fn i32_min_div_minus_one_wraps() {
        // Unoptimized so the division actually executes on the VM.
        let bc = bytecode_of(
            r#"
            fn main() -> i32 {
                let a = 0 - 2147483647 - 1;
                let b = 0 - 1;
                print_i32(a / b);
                print_i32(a % b);
                return 0;
            }
        "#,
        );
        let (_, out, _) = execute_captured(&bc).expect("vm");
        assert_eq!(out, "-2147483648\n0\n");
    }

    #[test]
    fn extern_call_is_a_vm_error() {
        let bc = bytecode_of(r#"extern fn foo(x: string); fn main() -> i32 { foo("x"); return 0; }"#);
        let (res, out, _) = run_captured(&bc);
        let err = res.unwrap_err().to_string();
        assert!(err.contains("extern function `foo`"), "{err}");
        assert_eq!(out, "");
    }

    #[test]
    fn host_fn_binds_extern() {
        let bc = bytecode_of(
            r#"
            extern fn twice(x: i32) -> i32;
            extern fn note(x: i32);
            fn main() -> i32 { note(twice(4)); return twice(twice(5)); }
        "#,
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let host: Vec<(String, HostFn)> = vec![
            ("twice".into(), Box::new(|a: &[Value]| Ok(Value::I32(a[0].as_i32() * 2)))),
            (
                "note".into(),
                Box::new(move |a: &[Value]| {
                    sink.lock().unwrap().push(a[0].as_i32());
                    Ok(Value::Unit)
                }),
            ),
        ];
        let (res, out, _) = execute_captured_with(&bc, VmOptions::default(), host);
        assert_eq!(res.unwrap(), Value::I32(20));
        assert_eq!(out, "");
        assert_eq!(*seen.lock().unwrap(), vec![8]);
    }

    #[test]
    fn run_captured_keeps_partial_stdout() {
        let bc = bytecode_of("fn main() -> i32 { print_i32(1); let z = 0; print_i32(5 / z); return 0; }");
        let (res, out, steps) = run_captured(&bc);
        assert_eq!(res.unwrap_err().to_string(), "division by zero");
        assert_eq!(out, "1\n");
        assert!(steps > 0);
        assert!(execute_captured(&bc).is_err());
    }

    #[test]
    fn cmp_rejects_mismatched_tags() {
        let err = cmp_values(CmpOp::Eq, &Value::I32(1), &Value::Str("1".into())).unwrap_err();
        assert_eq!(err.to_string(), "cannot compare i32 with string");
        assert!(cmp_values(CmpOp::Ne, &Value::F64(f64::NAN), &Value::F64(1.0)).unwrap());
        assert!(!cmp_values(CmpOp::Lt, &Value::F64(f64::NAN), &Value::F64(1.0)).unwrap());
    }

    #[test]
    fn many_registers_fit_in_u16() {
        let mut src = String::from("fn main() -> i32 {\n");
        for i in 0..300 {
            src.push_str(&format!("    let v{i} = {i} + 1;\n"));
        }
        src.push_str("    let mut t = 0;\n");
        for i in 0..300 {
            src.push_str(&format!("    t = t + v{i};\n"));
        }
        src.push_str("    print_i32(t);\n    return t;\n}\n");
        let bc = bytecode_of(&src);
        let (v, out, _) = execute_captured(&bc).expect("vm");
        assert_eq!(v, Value::I32(45150));
        assert_eq!(out, "45150\n");
    }

    #[test]
    fn budget_resumes_and_equals_run() {
        let bc = bytecode_of(
            r#"
            fn fib(n: i32) -> i32 {
                if n < 2 { return n; }
                return fib(n - 1) + fib(n - 2);
            }
            fn main() -> i32 {
                let mut i = 0;
                while i < 12 { print_i32(fib(i)); i = i + 1; }
                return fib(12);
            }
        "#,
        );
        let (whole, whole_out, whole_steps) = run_captured(&bc);
        let whole = whole.unwrap();

        let slot = Arc::new(Mutex::new(Vec::new()));
        let mut vm = Vm::new(&bc, VmOptions::default()).with_stdout(Box::new(Sink(slot.clone())));
        let mut yields = 0;
        let value = loop {
            match vm.run_budget(97).unwrap() {
                Step::Finished(v) => break v,
                Step::Yielded => {
                    yields += 1;
                    assert!(!vm.is_finished());
                    assert!(vm.steps() == 97 * yields, "budget must be exact");
                }
            }
        };
        assert!(yields > 1);
        assert!(vm.is_finished());
        assert_eq!(value, whole);
        assert_eq!(vm.steps(), whole_steps);
        assert_eq!(String::from_utf8_lossy(&slot.lock().unwrap()), whole_out);
        // Finished VMs keep answering with the same value.
        assert_eq!(vm.run_budget(1).unwrap(), Step::Finished(whole.clone()));
        assert_eq!(vm.run().unwrap(), whole);
    }

    #[test]
    fn budget_honours_total_step_limit() {
        let bc = bytecode_of("fn main() -> i32 { let mut i = 0; while true { i = i + 1; } return i; }");
        let opts = VmOptions { max_steps: 1_000, ..VmOptions::default() };
        let mut vm = Vm::new(&bc, opts.clone());
        let mut n = 0;
        let err = loop {
            match vm.run_budget(100) {
                Ok(Step::Yielded) => n += 1,
                Ok(Step::Finished(v)) => panic!("finished with {v}"),
                Err(e) => break e,
            }
        };
        assert!(matches!(err, VmError::StepLimit));
        assert_eq!(n, 10);
        assert_eq!(vm.steps(), 1_001);
        assert!(!vm.is_finished());
        assert!(vm.run_budget(1).is_err(), "halted vm stays halted");
        assert!(matches!(Vm::new(&bc, opts).run().unwrap_err(), VmError::StepLimit));
    }

    #[test]
    fn yield_op_yields_and_run_ignores_it() {
        let module = BytecodeModule {
            functions: vec![BcFunction {
                name: "main".into(),
                arity: 0,
                nregs: 1,
                code: vec![
                    Op::LoadImm { dest: 0, imm: Immediate::I32(1) },
                    Op::Yield,
                    Op::LoadImm { dest: 0, imm: Immediate::I32(2) },
                    Op::Ret { src: 0 },
                ],
                is_native: false,
                native_id: None,
                ret_ty: None,
            }],
            strings: vec![],
            entry: 0,
        };
        assert_eq!(Op::Yield.to_string(), "yield");
        let mut vm = Vm::new(&module, VmOptions::default());
        assert_eq!(vm.run_budget(1_000).unwrap(), Step::Yielded);
        assert_eq!(vm.steps(), 2, "yield counts as an instruction");
        assert!(!vm.is_finished());
        assert_eq!(vm.run_budget(1_000).unwrap(), Step::Finished(Value::I32(2)));
        assert!(vm.is_finished());
        assert_eq!(vm.steps(), 4);
        let mut plain = Vm::new(&module, VmOptions::default());
        assert_eq!(plain.run().unwrap(), Value::I32(2));
        assert_eq!(plain.steps(), 4);
    }

    #[test]
    fn array_index_reads_are_linear() {
        let n = 20_000;
        let mut src = String::from("fn main() -> i32 {\n    let xs = [");
        for i in 0..n {
            if i > 0 {
                src.push_str(", ");
            }
            src.push('1');
        }
        src.push_str(&format!(
            "];\n    let mut i = 0;\n    let mut s = 0;\n    while i < {n} {{\n        s = s + xs[i];\n        i = i + 1;\n    }}\n    return s;\n}}\n"
        ));
        let bc = bytecode_of(&src);
        let t = std::time::Instant::now();
        let (v, _, steps) = execute_captured(&bc).expect("vm");
        let elapsed = t.elapsed();
        assert_eq!(v, Value::I32(n));
        assert!(steps > n as u64 && steps < 20 * n as u64, "steps = {steps}");
        assert!(elapsed.as_secs_f64() < 2.0, "20k-element sum took {elapsed:?} (O(n^2) regression?)");
    }

    // --- copy-on-write aggregates -------------------------------------

    #[test]
    fn cow_clone_shares_until_written() {
        let a = Value::array(vec![Value::I32(1), Value::I32(2)]);
        let mut b = a.clone();
        if let (Value::Array(x), Value::Array(y)) = (&a, &b) {
            assert!(Rc::ptr_eq(x, y), "clone must share the buffer");
        }
        if let Value::Array(xs) = &mut b {
            Rc::make_mut(xs)[0] = Value::I32(9);
        }
        assert_eq!(a, Value::array(vec![Value::I32(1), Value::I32(2)]));
        assert_eq!(b, Value::array(vec![Value::I32(9), Value::I32(2)]));
        assert_eq!(a.to_string(), "[1, 2]");
        assert_eq!(Value::object(vec![Value::I32(1)]).to_string(), "{1}");
        assert_eq!(format!("{:?}", Value::array(vec![])), "Array([])");
    }

    #[test]
    fn cow_shared_array_is_not_mutated_through_the_other_handle() {
        let src = r#"
            fn poke(a: [i32; 3]) -> i32 { let mut c = a; c[1] = 50; return c[1]; }
            fn main() -> i32 {
                let a = [1, 2, 3];
                let mut b = a;
                let c = b;
                b[0] = 9;
                let n = poke(a);
                print_i32(a[0]); print_i32(b[0]); print_i32(c[0]);
                print_i32(a[1]); print_i32(n);
                return a[0] + b[0];
            }
        "#;
        for level in [0u8, 2u8] {
            let (v, out) = run_src_at(src, level);
            assert_eq!(v, Value::I32(10), "-O{level}");
            assert_eq!(out, "1\n9\n1\n2\n50\n", "-O{level}");
        }
    }

    #[test]
    fn cow_nested_array_write_back_still_works() {
        let src = r#"
            fn main() -> i32 {
                let mut g = [[1, 2], [3, 4]];
                let snap = g;
                let mut row = g[1];
                g[1][0] = 30;
                row[1] = 40;
                g[0] = row;
                print_i32(g[0][0]); print_i32(g[0][1]); print_i32(g[1][0]); print_i32(g[1][1]);
                print_i32(snap[0][0]); print_i32(snap[1][0]); print_i32(row[0]);
                return g[0][1] + snap[1][1];
            }
        "#;
        for level in [0u8, 2u8] {
            let (v, out) = run_src_at(src, level);
            assert_eq!(v, Value::I32(44), "-O{level}");
            assert_eq!(out, "3\n40\n30\n4\n1\n3\n3\n", "-O{level}");
        }
    }

    #[test]
    fn cow_struct_in_array_in_struct() {
        let src = r#"
            struct In { v: i32 }
            struct Out { xs: [In; 2], k: i32 }
            fn main() -> i32 {
                let mut o = Out { xs: [In { v: 1 }, In { v: 2 }], k: 7 };
                let keep = o;
                let mut inner = o.xs[1];
                inner.v = 20;
                o.xs[1] = inner;
                o.xs[0].v = 10;
                o.k = 8;
                print_i32(o.xs[0].v); print_i32(o.xs[1].v); print_i32(o.k);
                print_i32(keep.xs[0].v); print_i32(keep.xs[1].v); print_i32(keep.k);
                return o.xs[0].v + keep.xs[1].v;
            }
        "#;
        for level in [0u8, 2u8] {
            let (v, out) = run_src_at(src, level);
            assert_eq!(v, Value::I32(12), "-O{level}");
            assert_eq!(out, "10\n20\n8\n1\n2\n7\n", "-O{level}");
        }
    }
}

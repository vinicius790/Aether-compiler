# Changelog

All notable changes to this project are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
for the **source language and library API**. Bytecode and LLVM text are
explicitly unstable.

## [0.3.0] — 2026-10-02

### Added
- Nested variant/literal/tuple patterns, `let (a, (b, c)) = t;`; `match` as an expression
  (arm values, E0273, never-typed diverging arms); matrix exhaustiveness with a witness in
  E0270; warning W0272 (unreachable arm); E0274 (name bound twice in a pattern); `==` on arrays
- E0271 now only for identical variant patterns; E0268 only for float / refutable `let` patterns;
  unknown string escapes (E0006) and `''` (E0003) are errors
- Fixed: `for` bound re-read each iteration; liveness fixpoint capped at 64 rounds (DCE dropped
  live values in deep expressions); DCE removed unused trapping ops (`5 / z`, `a[5]`) at -O1/-O2;
  quadratic IR emission (40k-arm match 17 s → 0.6 s)
- `pub` is enforced across files (`E0281 ... is private to FILE`); stdlib API marked `pub`;
  `driver::compile_sources_public` for hosts such as the REPL
- `run --backend llvm` treats `main`'s value as the result (`driver::run_llvm_ir`, `LliStatus`)
- Colour only on a TTY (honours `NO_COLOR`, `--color` / `--no-color`); strict CLI option parsing;
  UTF-8 BOM ignored and a clear non-UTF-8 error; diagnostic carets align with tabs and multibyte text
- fmt fixes: main file only, keeps `let x: T`, prints `3.0` correctly, idempotent, syntax errors fail
- REPL: `enum`/`pub`/`use` inputs, bare expressions, `main_*` is no longer mistaken for `main`
- File modules: `use "relative/path.ae";` imports every item of another
  file (path relative to the importing file, `.ae` optional, transitive,
  each file included once, cycles allowed); `--include` / `AETHER_INCLUDE`
  are implicit `use`s. Missing import: `E0280 unresolved import`. `pub` is
  accepted before `fn` / `struct` / `extern fn` and recorded (`is_pub`), not
  enforced
- stdlib: `stdlib/vec2.ae` (`Vec2`, `vec2_*`) and `stdlib/rng.ae`
  (`rng_next`, `rng_range`, xorshift32) as importable files;
  `examples/modules.ae`; `tests/modules.rs`
- Enums with positional payloads: `enum Shape { Circle(f64), Rect(i32, i32), Empty }`,
  construction `Shape::Circle(1.5)` / `Shape::Empty`, `Type::Enum`; laid out as
  an object with the tag at field 0 and payload slots after it
- `match` statement with variant, literal (`i32`/`i64`/`bool`/`char`/`string`),
  binding and `_` patterns; exhaustiveness (E0270) and duplicate-arm (E0271)
  checks; arms may `return` / `break` / `continue`; `if let Pat = e { } else { }`
- Tuples `(T1, T2, ...)`: `(a, b)`, `t.0`, assignable fields,
  `let (a, b) = t;`, `Type::Tuple`
- Elementwise `==` / `!=` on tuples, enums and structs (lowered to a
  short-circuit chain in the IR; struct `==` previously failed with E0300 on the VM)
- Tokens `enum`, `match`, `::`, `=>`; new error codes E0204–E0206, E0265–E0271
- `examples/shapes.ae` (+ goldens), `tests/enums_tuples.rs`

## [0.2.2] — 2026-10-02

### Added
- Compound assignment (`+= -= *= /= %= &= |= ^= <<= >>=`), bitwise `& | ^ << >>`
  and integer `!`, literals `0x`/`0b`/`0o`/`1_000`, `\u{...}` escapes
- Built-ins `print_char`, `to_string`, `i64_to_string`, `f64_to_string`,
  `char_to_string`, `abs`, `min`, `max`, `clamp`, `sqrt`, `floor`, `ceil`,
  `pow_i32`; `len` accepts arrays; a user `fn` shadows a built-in
- `yield;` statement, `Vm::run_budget` / `Step` for per-frame execution
  budgets, `Op::Yield`
- Host-bindable `extern fn` (`host::Host::register`, `Vm::with_host_fn`)
- CLI: `--include FILE` / `AETHER_INCLUDE` multi-file programs and
  `stdlib/prelude.ae`, `--max-steps`, `--max-depth`, `run --backend llvm`,
  `bench <file> [--n N]`, REPL that keeps definitions (`:items`, `:reset`)
- Optimizer: fixpoint driver, `dead-fn` (unreachable functions removed),
  `regalloc` register compaction (type-aware), bitwise identities
- Golden tests for example IR/bytecode (`UPDATE_GOLDENS=1`), suites
  `language_ops`, `cli_features`, `host_api`, `yield_stmt`
- Fuzz kind `agg` (alias `aggregate`, part of `all`): well-typed programs with
  structs, nested arrays, `i64`/`f64`/`char`/`string`, casts, `&&`/`||`
  guards, nested assignment and tail-expression functions, checked by the
  `-O0` / `-O2` differential oracle
- `tests/regressions.rs`: one test per fixed bug, each program run at `-O0`
  and `-O2` (both must agree and match the expected stdout/value);
  rows R1–R22 and the `agg` fuzz row in `docs/test-matrix.md`
- VM opcodes: generic `Cmp { op: Eq|Ne|Lt|Le|Gt|Ge }` for `i64`/`f64`/`bool`/
  `char`/`string` comparisons not covered by the specialised ops, `RemI64`,
  `NegI64`, `CastI64ToF64`, `CastF64ToI64`, `CastBoolToI64`, `CastCharToI32`,
  `CastI32ToChar`
- Diagnostics: E0221 (tail expression type differs from the return type),
  E0238 (`for` bounds must be `i32`), E0263 (integer literal out of `i32`
  range), E0264 (function name used as a value), E0300 (unsupported
  operator/type combination in the assembler), E0101 for nesting deeper than
  256 (expressions or blocks)
- Constant folding of `i64` / `f64` / `char` / `string` / `bool` comparisons
  and of `i64` `%`

### Changed
- The trailing expression (no `;`) of a function body is the return value and
  must match the return type; a trailing expression in any nested block is
  evaluated as a statement
- Negative literals are single literals (`let y: i64 = -1;`, `-2147483648`
  is a valid `i32`); the expected type propagates through unary minus,
  parentheses and arithmetic/comparison operators, and a bare integer literal
  adopts the type of the other operand (`1 + a` with `a: i64` is `i64`)
- Struct literals may list fields in any order (evaluated in source order,
  stored at the declared position)
- Nested assignment (`a[i][j] = v`, `o.inner.x = v`) is supported
- `&&` / `||` short-circuit: the right operand runs only when needed
- Arrays and structs have value semantics: `let b = a;` copies (deep copy
  in the VM)
- Integer division wraps (`i32::MIN / -1 == i32::MIN`) in the VM and in the
  optimizer
- `len` counts Unicode scalar values (chars), consistent with indexing
- Every comparison and cast that sema accepts now executes (`i64` all six
  comparisons, `f64` all six, `bool` `==` `!=`, `char` all six, `string`
  `==` `!=`; `i64` `%` and negation; all casts in `ty.rs::can_cast_to`)
- VM registers are `u16` (up to 65535 per frame); the assembler reports a
  compile error instead of truncating
- `-O2` pass order: const-fold, algebraic, cf-simplify, inline, local-cse,
  copy-prop, const-prop, cf-simplify, dce, const-fold, dce (cf-simplify now
  runs before inline)
- Inliner copies arguments into fresh registers and remaps every instruction
  kind
- Algebraic identities (`x-x`, `x*0`, `x==x`, `x+0`, `x*1`, `x/1`, `x*2`)
  apply to `i32` and `i64` only
- copy-prop no longer redirects the base of an element/field store and drops
  aliases on both sides of a store
- The CLI prints the stdout produced so far before `runtime error: ...`
- LLVM emitter produces valid LLVM 18 IR (one `alloca` per IR register,
  `load`/`store`, ready for `mem2reg`; accepted by `llvm-as` on every
  example). Known differences from the VM: aggregates are stack-allocated and
  copying an array/struct copies the pointer (aliasing), no bounds checks,
  string concatenation is unsupported (aborts), `print_f64` uses `%g`
- Fuzz: the junk, format and mutation properties also compile at `-O2`
- Version 0.2.2 in `Cargo.toml`, `Cargo.lock`, README badge, `MANUAL.md`
  (README said 0.2.0 and `MANUAL.md` said 0.2.0 while this file had 0.2.1)

### Fixed
Miscompilations:
- Trailing expression of a function body was discarded
- Struct literal with fields in non-declaration order stored them at the
  wrong positions
- Nested assignment (`a[i][j] = v`, `o.inner.x = v`) was lost
- The `for` variable leaked out of the loop body scope
- `&&` / `||` evaluated both operands
- `-O2` copy-prop aliased array/struct copies (`let b = a;`)
- `x-x`, `x*0`, `x==x` were folded for `f64`, wrong for NaN
- More than 255 registers per function were silently truncated
- Integer literals out of `i32` range silently wrapped
- Missing opcodes (`i64` comparisons beyond `Eq`/`Lt`, `i64` `%`, several
  casts) were emitted as `Nop`, so the result was `()`
- `i64` negation was truncated to 32 bits
- Calling an `extern fn` the VM does not implement behaved like `print`
- The inliner never fired: lowering leaves a dead block after every `return`,
  so no function looked like a single-block leaf

Crashes and usability:
- `i32::MIN / -1` panicked in the VM and in `-O2` constant folding
- Deeply nested input overflowed the parser stack (now diagnosed, E0101)
- Stdout produced before a runtime error was lost
- `len` counted bytes instead of chars

## [0.2.1] — 2026-09-19

### Added
- `aether profile` / `aether digest` (call counts + FNV fingerprint)
- Liveness analysis (`dump-liveness`, `stats`)
- Leaf inlining and local CSE
- Algebraic `&&` / `||` / `x/1`
- Liveness-driven DCE
- `dump-hir`, examples `cse.ae` / `inline.ae`

## [0.2.0] — 2026-09-19

### Added
- IR structural verifier (`aether verify`, `src/ir/verify.rs`)
- Graphviz CFG export (`aether cfg`, `src/ir/cfg.rs`)
- AST pretty-printer (`aether fmt`, `src/pretty.rs`)
- In-tree greybox fuzzer with VM edge coverage
- Format-based fuzzer driven by the Aether EBNF
- Direct IR generation with decoy blocks and opaque predicates
- Algebraic fold of opaque predicates (`x-x`, `x==x`, `x*0`)
- Aspect-preserving and structural mutation
- Reference routines in `stdlib/math.ae`
- Example `examples/opaque.ae`

### Changed
- Optimizer algebraic pass kills destinations on reassignment
- Fuzz CLI reports extra greybox statistics

### Security
- No known issues. See SECURITY.md.

## [0.1.0] — 2026-09-19

### Added
- Language 0.1, lexer, parser, semantic analysis
- Three-address IR, optimizer pipeline
- Register VM and textual LLVM backend
- Property-based differential testing of `-O0` vs `-O2`

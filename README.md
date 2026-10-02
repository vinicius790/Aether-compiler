# Aether

[![CI](https://img.shields.io/badge/CI-cargo%20test-0B6-blue)](.github/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT-informational)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.75-orange)](rust-toolchain.toml)
[![Version](https://img.shields.io/badge/version-0.2.2-lightgrey)](CHANGELOG.md)

A small, statically typed imperative language and a complete compiler
pipeline — lexer through optimizer through a register VM — plus an
optional textual LLVM backend.

```
Source → Lexer → Parser → AST → Sema / Types → IR → Optimizer
                                                 ↙          ↘
                                          Bytecode → VM     LLVM IR (text)
```

Aether is designed so every layer is inspectable and testable. The VM is
the execution contract. LLVM emission is for inspection and experiment,
not a production code generator: the text is valid LLVM 18 IR (checked with
`llvm-as` on every example), but aggregates alias and there are no bounds
checks (see [`docs/status.md`](docs/status.md)).

**Handbook (what exists and how to use it):** [`MANUAL.md`](MANUAL.md).

Language spec: [`docs/language.md`](docs/language.md) ·
Grammar: [`docs/grammar.md`](docs/grammar.md) ·
Status: [`docs/status.md`](docs/status.md) ·
Docs index: [`docs/README.md`](docs/README.md)

## Example

```
fn fib(n: i32) -> i32 {
    if n < 2 { return n; }
    return fib(n - 1) + fib(n - 2);
}

fn main() -> i32 {
    print_i32(fib(10));
    return 0;
}
```

Types: `i32`, `i64`, `f64`, `bool`, `string`, `char`, `unit`, `[T; N]`, `struct`.  
Control: `if`/`else`, `while`, `for i in a..b`, `return`, `break`, `continue`.  
Built-ins: `print`, `println`, `print_i32`, `print_i64`, `print_f64`, `print_bool`, `len`, `assert`.

## Requirements

- Rust 1.75+ (edition 2021). See `rust-toolchain.toml`.
- Optional: LLVM 18 (`opt-18`, `lli-18`) if you want to feed the textual backend to `opt`.

No crates.io dependencies. The compiler, VM, and fuzzer are std-only.

## Build

```bash
cargo build --release
cargo test
make examples
```

Binary: `target/release/aether`.

## Command line

```bash
aether run examples/fib.ae -O2 --stats --timings
aether run examples/fib.ae --include stdlib/prelude.ae   # multi-file (also AETHER_INCLUDE=a.ae:b.ae)
aether run examples/loops.ae --max-steps 100000 --max-depth 512
aether run examples/hello.ae --backend llvm              # needs `lli` on PATH
aether check examples/hello.ae
aether dump-ir examples/opt_demo.ae --unopt
aether dump-ir examples/opt_demo.ae -O2
aether optimize examples/opt_demo.ae
aether compile examples/hello.ae --emit llvm -o hello.ll
aether fmt examples/hello.ae
aether cfg examples/fib.ae
aether verify examples/opt_demo.ae -O2
aether stats examples/opt_demo.ae -O2
aether profile examples/fib.ae -O2
aether digest examples/fib.ae -O2
aether dump-hir examples/hello.ae
aether dump-liveness examples/fib.ae -O2
aether bench examples/fib.ae --n 5                       # -O0 vs -O2 on a file
aether repl                                              # stateful: :items :reset :quit
aether benchmark --n 20
aether fuzz --kind diff --iters 200
aether fuzz --kind agg --iters 200
aether fuzz --kind greybox --iters 40
aether fuzz --kind format --iters 80
```

Exit codes: `0` ok, `1` compile error, `2` runtime error (including an
exceeded `--max-steps` / `--max-depth`). On a runtime error the stdout
produced so far is printed before `runtime error: ...`.

Full tool list: [`docs/tools.md`](docs/tools.md) · [`docs/cli.md`](docs/cli.md).

## Optimizer

Passes on Aether IR (`-O1` / `-O2`):

- constant folding and propagation (also `i64`/`f64`/`char`/`string`/`bool`
  comparisons and `i64` `%`; division wraps)
- algebraic identities, including opaque predicates (`x-x`, `x==x`, `x*0`);
  applied to `i32` and `i64` only, never to `f64` (NaN)
- local common-subexpression elimination
- inlining of single-block leaf functions
- copy propagation
- control-flow simplify (empty-block jump threading, unreachable delete)
- dead-code elimination

Order at `-O2`: const-fold, algebraic, cf-simplify, inline, local-cse,
copy-prop, const-prop, cf-simplify, dce, const-fold, dce.

`aether optimize file.ae` prints instruction counts **before → after**.  
`aether fuzz --kind diff` checks that `-O0` and `-O2` agree on value and stdout.

## Fuzzing

In-tree, deterministic, no nightly:

| Kind | What it exercises |
|------|-------------------|
| `lexer` `parser` `pipeline` | no-panic on junk |
| `gen` `diff` | well-typed programs; O0 ≡ O2 |
| `agg` | well-typed programs with structs, arrays, `i64`/`f64`/`char`/`string`, casts, `&&`/`\|\|` guards, nested assignment, tail-expression functions; O0 ≡ O2 |
| `mut` `struct` `aspect` | havoc / tree / type-preserving mutation |
| `mir` | direct IR generation, decoy blocks |
| `greybox` | VM edge coverage + energy schedule |
| `format` | EBNF choice-tape (grammar-as-format) |

`agg` (alias `aggregate`) is part of `all`. The junk, format and mutation
properties also compile at `-O2`.

See [`docs/fuzzing.md`](docs/fuzzing.md), [`docs/greybox.md`](docs/greybox.md), [`docs/aspect-mir.md`](docs/aspect-mir.md).

## Layout

```
src/          compiler library + CLI
  lexer.rs parser.rs ast.rs sema/ ir/ opt/ backend/ vm/ fuzz/
examples/     programs used as tests
stdlib/       reference routines (no module system yet)
benchmarks/   timing programs
docs/         specification and design notes
tests/        integration tests against the `aether` binary
              (cli_examples, tools, corpus, regressions)
.github/      CI and contribution templates
```

## Compatibility

- Source language version: **Aether 0.2**
- Bytecode is **not** stable across releases
- LLVM text is **not** a supported ABI

## Contributing

Please read [CONTRIBUTING.md](CONTRIBUTING.md), [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)
and [SECURITY.md](SECURITY.md). Design changes belong in `docs/` in the same
pull request as the code.

## License

MIT. Copyright 2026 Aether Contributors. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

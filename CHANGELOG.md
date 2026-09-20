# Changelog

All notable changes to this project are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
for the **source language and library API**. Bytecode and LLVM text are
explicitly unstable.

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

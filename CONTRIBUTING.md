# Contributing to Aether

Thank you. The project is small on purpose. A change that is not specified,
tested and documented does not land.

## Ground rules

1. Keep `docs/language.md` and `docs/grammar.md` aligned with the parser
   and the type checker in the same pull request.
2. A language feature needs: syntax, semantics, IR lowering, VM behaviour,
   a test, and a sentence in `docs/status.md`.
3. Do not add crates.io dependencies without a written justification in
   `docs/status.md`. The default answer is no.
4. `cargo test` must pass. Also run `make examples`.
5. Optimizer passes are intra-block or they must honour back-edges.
   Non-SSA means a write in one successor is not defined on the other.
6. The VM is the execution backend. LLVM text is not.

## Development

```bash
rustup override set 1.75.0   # or use rust-toolchain.toml
cargo test
cargo run -- fuzz --iters 40 --kind all --seed 1
```

MSRV is **1.75**. Edition 2021. No edition 2024 features.

## Fuzz findings

If a fuzzer prints a seed, add a regression test with that seed before
the fix. Do not silence the property.

## Commit messages

```
<area>: <imperative summary>

Optional body. Areas: lexer, parser, sema, ir, opt, vm, llvm, fuzz, docs, cli.
```

## Pull requests

Use the template in `.github/PULL_REQUEST_TEMPLATE.md`.
One concern per PR. Do not mix a language change with a fuzzer refactor.

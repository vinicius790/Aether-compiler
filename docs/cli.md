# CLI

Full command table: [`tools.md`](tools.md).

```
aether <comando> [opções] [arquivo]
```

| Comando | Função |
|---------|--------|
| `check FILE` | análise semântica |
| `run FILE -On --stats --timings` | compila e executa na VM |
| `compile FILE --emit ir\|bytecode\|llvm -o PATH` | emite artefato |
| `dump-tokens FILE` | tokens |
| `dump-ast FILE` | AST |
| `dump-ir FILE [--unopt] -On` | IR |
| `dump-bytecode FILE` / `disassemble FILE` | bytecode |
| `dump-llvm FILE` | LLVM IR textual |
| `optimize FILE` | relatório antes/depois |
| `repl` | REPL |
| `benchmark --n N` | Fibonacci `-O0` vs `-O2` |
| `fuzz [--iters N] [--seed N] [--kind …]` | propriedades / fuzz |
| `help` / `version` | meta |

Códigos de saída: `0` ok, `1` erro de compilação, `2` erro de runtime.

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
| `fmt FILE` | pretty-print a partir da AST |
| `cfg FILE [-On]` | Graphviz DOT do CFG da IR |
| `verify FILE [-On]` | verificador estrutural da IR |
| `dump-hir FILE` | HIR tipada |
| `dump-liveness FILE [-On]` | conjuntos live-in por bloco |
| `stats FILE [-On]` | relatório do otimizador + liveness + tamanho da IR |
| `profile FILE [-On]` | contagem de chamadas + digest da execução |
| `digest FILE [-On]` | impressão digital determinística de stdout + valor |
| `repl` | REPL |
| `benchmark --n N` | Fibonacci `-O0` vs `-O2` |
| `fuzz [--iters N] [--seed N] [--kind …]` | propriedades / fuzz (`all` inclui `agg`) |
| `help` / `version` | meta |

Códigos de saída: `0` ok, `1` erro de compilação, `2` erro de runtime.
Num erro de runtime a CLI imprime o stdout produzido até ali antes de
`runtime error: ...`.

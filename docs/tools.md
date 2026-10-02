# Ferramentas no repositório

Tudo vive no binário `aether` ou em `src/fuzz`. Não há crates extra.

| Comando | Função |
|---------|--------|
| `check` | só semântica |
| `run` | compila e executa na VM |
| `compile --emit ir\|bytecode\|llvm` | artefactos |
| `dump-tokens / dump-ast / dump-ir / dump-bytecode / dump-llvm` | inspecção |
| `optimize` | relatório antes/depois dos passes |
| `fmt` | pretty-print a partir da AST |
| `cfg` | Graphviz DOT do CFG da IR |
| `verify` | verificador estrutural da IR |
| `repl` | ciclo ler–compilar–correr |
| `benchmark` | `fib(n)` O0 vs O2 |
| `fuzz` | propriedades, mutação, greybox, formato |
| `dump-hir FILE` | HIR tipada |
| `dump-liveness FILE [-On]` | live-in por bloco |
| `stats FILE [-On]` | relatório do otimizador + liveness + tamanho da IR |
| `profile FILE [-On]` | contagem de chamadas + digest da execução |
| `digest FILE [-On]` | impressão digital determinística de stdout + valor |

Códigos de saída: `0` ok, `1` erro de compilação, `2` erro de runtime.

Fuzz kinds: `all`, `lexer`, `parser`, `pipeline`, `gen`, `diff`, `agg` (alias `aggregate`), `mut`, `struct`, `aspect`, `mir`, `greybox`, `format`. `all` inclui `agg`.

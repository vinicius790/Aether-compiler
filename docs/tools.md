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
| `dump-hir` | HIR tipada |
| `dump-liveness` | live-in por bloco |

Fuzz kinds: `all`, `lexer`, `parser`, `pipeline`, `gen`, `diff`, `mut`, `struct`, `aspect`, `mir`, `greybox`, `format`.

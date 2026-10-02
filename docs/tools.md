# Ferramentas no repositório

Tudo vive no binário `aether` ou em `src/fuzz`. Não há crates extra.

| Comando | Função |
|---------|--------|
| `check` | só semântica |
| `run [--backend vm\|llvm]` | compila e executa na VM (ou via `lli`) |
| `compile --emit ir\|bytecode\|llvm` | artefactos |
| `dump-tokens / dump-ast / dump-ir / dump-bytecode / dump-llvm` | inspecção |
| `optimize` | relatório antes/depois dos passes |
| `fmt` | pretty-print a partir da AST |
| `cfg` | Graphviz DOT do CFG da IR |
| `verify` | verificador estrutural da IR |
| `repl` | ciclo ler–compilar–correr com definições persistentes (`:items`, `:reset`, `:help`, `:quit`) |
| `bench FILE [--n N]` | `-O0` vs `-O2` sobre um ficheiro: µs min/mediana, passos da VM, instruções IR, speedup |
| `benchmark` | `fib(n)` O0 vs O2 |
| `fuzz` | propriedades, mutação, greybox, formato |
| `dump-hir FILE` | HIR tipada |
| `dump-liveness FILE [-On]` | live-in por bloco |
| `stats FILE [-On]` | relatório do otimizador + liveness + tamanho da IR |
| `profile FILE [-On]` | contagem de chamadas + digest da execução |
| `digest FILE [-On]` | impressão digital determinística de stdout + valor |

Opções transversais (detalhe em [`cli.md`](cli.md)):

| Opção | Efeito |
|-------|--------|
| `--include FILE` / `-I FILE` / `--include=FILE` (repetível), `AETHER_INCLUDE=a.ae:b.ae` | vários ficheiros num só programa |
| `--max-steps N` / `--max-depth N` | limites da VM em `run`, `profile`, `digest`, `bench`, `benchmark`, `repl` |
| `--backend vm\|llvm` | motor de `run` |

Códigos de saída: `0` ok (também `help` e `--help`), `1` erro de compilação,
uso errado da CLI (incluindo `aether` sem comando) ou falha do `fuzz`, `2`
erro de runtime (incluindo limites da VM excedidos).

`make bench` corre `aether bench` sobre `benchmarks/*.ae`.

Fuzz kinds: `all`, `lexer`, `parser`, `pipeline`, `gen`, `diff`, `agg` (alias `aggregate`), `lang`, `mut`, `struct`, `aspect`, `mir`, `greybox`, `format`. `all` corre todos menos `greybox` e `format`.

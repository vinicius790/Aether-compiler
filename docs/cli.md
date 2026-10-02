# CLI

Full command table: [`tools.md`](tools.md).

```
aether <comando> [opções] [arquivo]
```

| Comando | Função |
|---------|--------|
| `check FILE` | análise semântica |
| `run FILE -On --stats --timings [--backend vm\|llvm]` | compila e executa na VM (ou via `lli`) |
| `compile FILE --emit ir\|bytecode\|llvm -o PATH` | emite artefato |
| `dump-tokens FILE` | tokens |
| `dump-ast FILE` | AST |
| `dump-ir FILE [--unopt] -On` | IR |
| `dump-bytecode FILE` / `disassemble FILE` | bytecode |
| `dump-llvm FILE` | LLVM IR textual |
| `optimize FILE` | relatório antes/depois |
| `fmt FILE` | pretty-print a partir da AST (só o ficheiro principal) |
| `cfg FILE [-On]` | Graphviz DOT do CFG da IR |
| `verify FILE [-On]` | verificador estrutural da IR |
| `dump-hir FILE` | HIR tipada |
| `dump-liveness FILE [-On]` | conjuntos live-in por bloco |
| `stats FILE [-On]` | relatório do otimizador + liveness + tamanho da IR |
| `profile FILE [-On]` | contagem de chamadas + digest da execução |
| `digest FILE [-On]` | impressão digital determinística de stdout + valor |
| `bench FILE [--n N]` | `-O0` vs `-O2`: µs min/mediana, passos da VM, instruções IR, speedup |
| `repl [-On]` | REPL com estado (`:items`, `:reset`, `:quit`) |
| `benchmark --n N` | Fibonacci `-O0` vs `-O2` |
| `fuzz [--iters N] [--seed N] [--kind …]` | propriedades / fuzz (`all` inclui `agg`) |
| `help` / `version` | meta |

## Opções

| Opção | Comandos | Efeito |
|-------|----------|--------|
| `-On` / `-O n` | todos os que compilam | nível do otimizador (omissão: 2) |
| `--include FILE` (repetível) | check, run, compile, dump-*, optimize, verify, cfg, stats, profile, digest, bench | compila `FILE` junto com o ficheiro principal (ver abaixo) |
| `--max-steps N` | run, profile, digest, bench | orçamento de instruções da VM (omissão: 50 000 000) |
| `--max-depth N` | run, profile, digest, bench | profundidade máxima de chamadas (omissão: 10 000) |
| `--backend vm\|llvm` | run | `vm` (omissão) executa o bytecode; `llvm` emite LLVM IR e corre-o com `lli` |
| `--timings` / `--stats` | run | tempos por fase / valor, passos e relatório do otimizador (stderr) |
| `--unopt` | dump-ir | IR antes dos passes |
| `--emit`, `-o` | compile | artefacto e ficheiro de saída |
| `--n N` | bench (omissão 5), benchmark (omissão 20) | repetições / argumento de `fib` |
| `--iters`, `--seed`, `--kind` | fuzz | configuração do fuzzer |

Códigos de saída: `0` ok, `1` erro de compilação, `2` erro de runtime.
Num erro de runtime a CLI imprime o stdout produzido até ali antes de
`runtime error: ...`. Exceder `--max-steps` ou `--max-depth` é um erro de
runtime (`exit 2`) e o stdout parcial é preservado.

## Vários ficheiros: `--include`

A linguagem não tem `mod`/`use`. Em vez disso o *driver* aceita vários
ficheiros: cada um recebe o seu `FileId` na sessão e é lexado em separado
(os diagnósticos apontam para o ficheiro certo), os tokens são concatenados
e o resultado é analisado como um único programa. Nomes repetidos entre
ficheiros dão o erro habitual `duplicate function`.

```bash
aether run game.ae --include stdlib/prelude.ae
aether check main.ae --include a.ae --include b.ae
AETHER_INCLUDE=stdlib/prelude.ae aether run game.ae   # includes por omissão
```

`AETHER_INCLUDE` é uma lista separada por `:`; as entradas vêm antes dos
`--include` da linha de comando. `stdlib/prelude.ae` não tem `fn main` e
existe para ser incluído desta forma ([`stdlib.md`](stdlib.md)).

## `run --backend llvm`

Emite o LLVM IR textual (o mesmo de `dump-llvm`) e executa-o com
`opt`/`lli` (`lli-18` ou `lli` no `PATH`), imprimindo o stdout do programa.
Sem `lli` a CLI imprime um erro claro e sai com `1`. Sob `lli` o valor
devolvido por `main` é o código de saída do processo, por isso um `main`
que devolve ≠ 0 é reportado como falha (`exit 1`).

## `bench`

```
$ aether bench examples/fib.ae --n 5
bench examples/fib.ae  runs=5
  -O0: exec_us min=234 median=328  vm_steps=1506  ir_insts=16  result=55
  -O2: exec_us min=216 median=244  vm_steps=1416  ir_insts=12  result=55
  speedup -O2 vs -O0: 1.34x exec (median), 1.06x vm steps, 1.33x ir insts
```

Compila em `-O0` e `-O2`, corre cada nível `N` vezes (omissão 5) e compara.
Se os dois níveis produzirem valor/stdout diferentes imprime um aviso.
`make bench` corre-o sobre `benchmarks/*.ae`.

## REPL

```
$ aether repl
aether> fn sq(x: i32) -> i32 { return x * x; }
aether>                                   ← linha vazia executa
defined sq
aether> print_i32(sq(7));
aether>
49
=> 0
```

- Uma entrada termina numa linha vazia.
- Entradas que começam por `fn`, `struct` ou `extern` são **definições**:
  ficam guardadas para as entradas seguintes (redefinir um nome substitui a
  versão anterior). Um erro numa definição nova descarta só essa entrada —
  as anteriores mantêm-se.
- Qualquer outra entrada é um bloco de instruções embrulhado num `main`
  novo e executado contra as definições guardadas; a CLI imprime o stdout e
  `=> valor`.
- Uma entrada com `fn main` corre como programa completo (nada é guardado).
- `:items` lista os nomes guardados, `:reset` esquece-os, `:help` ajuda,
  `:quit` (ou `:exit`) sai. `-On` e `--max-steps`/`--max-depth` aplicam-se
  a cada execução.

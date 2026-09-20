# Matriz de testes Aether 0.2

Cada linha é um contrato. Se o comando à direita falhar, a versão está
partida. Isto não substitui `cargo test`; organiza o que o CI e um
humano devem conseguir reproduzir.

## Front-end

| # | Fonte | Esperado |
|---|--------|----------|
| L1 | `dump-tokens examples/hello.ae` | tokens + Eof |
| L2 | ficheiro vazio | Eof só, sem panic |
| L3 | comentário `//` | tokens do resto |
| P1 | `dump-ast examples/fib.ae` | `Fn` fib e main |
| P2 | `check` em programa sem main | erro semântico |
| P3 | `check tests` fixture má | exit 1 |
| S1 | `check examples/structs.ae` | ok |
| S2 | tipo `i32` vs `bool` | erro |
| S3 | nome inexistente | erro |

## IR e opt

| # | Comando | Esperado |
|---|---------|----------|
| I1 | `dump-ir examples/opt_demo.ae --unopt` | add visível |
| I2 | `dump-ir examples/opt_demo.ae -O2` | menos insts |
| I3 | `optimize examples/opt_demo.ae` | delta negativo ou zero |
| I4 | `verify examples/fib.ae` | `verify: ok` |
| I5 | `cfg examples/fib.ae` | `digraph` |
| I6 | `examples/opaque.ae -O2` | valor 1, sem `br` no predicado |
| I7 | `examples/cse.ae -O2` | corre |
| I8 | `examples/inline.ae -O2` | 42 |
| I9 | `dump-liveness examples/fib.ae` | `function` |
| I10 | `dump-hir examples/hello.ae` | `HirProgram` |

## Execução

| # | Programa | Valor / stdout |
|---|----------|----------------|
| E1 | hello | corre |
| E2 | fib | print 55 |
| E3 | loops | soma |
| E4 | opt_demo | print 19 |
| E5 | math.ae | quatro prints |
| E6 | `profile fib -O0` | `fib` com calls > 1 |
| E7 | `digest hello` | hex estável na mesma build |

## Fuzz

| Kind | Iters mínimas smoke |
|------|---------------------|
| all | 40 |
| format | 20 |
| greybox | 16 |
| diff | 50 |
| mir | 30 |
| aspect | 30 |

Semente de smoke no CI: `1`.

## Corpus

`tests/corpus.rs` corre um subconjunto de `corpus/cNNN.ae` em `-O2`.
Todos os `cNNN.ae` presentes nesse subconjunto têm de sair 0.

## Goldens

`goldens/*.ir.O0.txt` e `*.ir.O2.txt` são dumps de referência para
diff manual. Não estão assertados byte-a-byte no CI porque a IR
textual ainda pode ganhar nomes de bloco. Use-os em review.

## Não-regressão conhecida

| Bug | Teste |
|-----|-------|
| fold através de join não-SSA | `does_not_fold_across_reassignment` |
| shrink a apagar helper chamado | `uses_in_expr` no sketch |
| overflow do pretty wrap | `map_leaves` post-order |
| `check_ir` ids após retain | existência por id, não índice |

# Matriz de testes Aether

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
| agg | 30 |
| lang | 30 |

Semente de smoke no CI: `1`. `tests/fuzz_smoke.rs` corre 30 casos por kind com seeds
fixas (e verifica que o gerador `lang` alcança cada construção nova).

Oráculos (kinds `agg`, `lang`, `diff`): -O0 == -O1 == -O2, `verify_module`,
determinismo, `run_budget` (1, 7, 1000) == `run`, `fmt` idempotente (ver
[fuzzing.md](fuzzing.md)), watchdog de 5 s por caso.

## Corpus

`tests/corpus.rs` corre um subconjunto de `corpus/cNNN.ae` em `-O2`.
Todos os `cNNN.ae` presentes nesse subconjunto têm de sair 0.

## Goldens

`goldens/*.ir.O0.txt` e `*.ir.O2.txt` são dumps de referência para
diff manual. Não estão assertados byte-a-byte no CI porque a IR
textual ainda pode ganhar nomes de bloco. Use-os em review.

## Regressões 0.2.2 (`tests/regressions.rs`)

Um teste por bug corrigido. Cada programa corre em `-O0` e `-O2`; os dois
têm de concordar e coincidir com o stdout/valor esperado.

| # | Caso | Esperado |
|---|------|----------|
| R1 | expressão final (sem `;`) do corpo de uma função | é o valor devolvido |
| R2 | literal de struct com campos fora da ordem declarada | campos nas posições declaradas, avaliados pela ordem do fonte |
| R3 | atribuição aninhada `a[i][j] = v`, `o.inner.x = v` | escrita visível depois |
| R4 | variável de `for` | só existe no corpo do laço |
| R5 | `&&` / `\|\|` com operando direito com efeito | só avaliado quando necessário |
| R6 | `let b = a;` com array, depois mutar `b` | `a` inalterado (semântica de valor, também em `-O2`) |
| R7 | `x - x`, `x * 0`, `x == x` com `f64` NaN | não são dobrados; resultado de NaN preservado |
| R8 | função com mais de 255 registradores | compila e corre (registradores `u16`) |
| R9 | literal inteiro fora do intervalo de `i32` | erro E0263; `-2147483648` é válido |
| R10 | todas as comparações aceites pela sema (`i64`, `f64`, `bool`, `char`, `string`) | executam com o resultado correcto |
| R11 | negação de `i64` | não trunca a 32 bits |
| R12 | todas as conversões de `can_cast_to` | executam com o resultado correcto |
| R13 | chamada de `extern fn` que a VM não implementa | erro de runtime (não imprime) |
| R14 | `i32::MIN / -1` | `i32::MIN`, sem pânico, em `-O0` e `-O2` |
| R15 | aninhamento acima de 256 | diagnóstico E0101, sem estouro de pilha |
| R16 | erro de runtime depois de `print` | CLI imprime o stdout parcial e depois `runtime error: ...` (exit 2) |
| R17 | `len` de string com caracteres não ASCII | conta caracteres, não bytes |
| R18 | literais negativos e tipo esperado (`let y: i64 = -1;`, `1 + a` com `a: i64`) | `i64` |
| R19 | nome de função usado como valor | erro E0264 |
| R20 | limites de `for` que não são `i32` | erro E0238 |
| R21 | tipo da expressão final diferente do tipo de retorno | erro E0221 |
| R22 | folha de um bloco chamada de outra função em `-O2` | inlinada (a chamada desaparece da IR) |

## Não-regressão conhecida

| Bug | Teste |
|-----|-------|
| fold através de join não-SSA | `does_not_fold_across_reassignment` |
| shrink a apagar helper chamado | `uses_in_expr` no sketch |
| overflow do pretty wrap | `map_leaves` post-order |
| `check_ir` ids após retain | existência por id, não índice |

## Auditorias

| Teste | Contrato |
|-------|----------|
| `tests/audit_match.rs` | match/enum/tuplas/padrões aninhados: -O0 = -O2 = esperado |
| `tests/audit_lang2.rs` | agregados zerados, `[v; N]`, arrays vazios, `i64::MIN`, `pub enum`, nomes privados por ficheiro |
| `tests/audit_vm.rs` | núcleo de execução: valores, aritmética, strings, limites, `run_budget`, host |
| `tests/audit_modules.rs` | `use`, `pub` (E0281), `fmt`, entradas estranhas, diagnósticos, CLI |
| `tests/audit_llvm.rs` | LLVM (`lli`) = VM em stdout e em falhas (saltado sem `lli`) |
| `tests/audit_a3.rs` | cada linha de `docs/diagnostics.md` (código e mensagem), erros de uso da CLI, REPL, `benchmark`, `dump-tokens`, versões iguais em todos os ficheiros |

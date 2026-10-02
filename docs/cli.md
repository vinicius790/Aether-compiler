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
| `repl [-On]` | REPL com estado (`:items`, `:reset`, `:help`, `:quit`) |
| `benchmark --n N` | Fibonacci `-O0` vs `-O2` |
| `fuzz [--iters N] [--seed N] [--kind …]` | propriedades / fuzz (`all` inclui `agg`) |
| `help` / `version` | meta (também `-h`/`--help` depois de qualquer comando, `--version`/`-V`) |

## Opções

| Opção | Comandos | Efeito |
|-------|----------|--------|
| `-On` / `-O n` | todos os que compilam | nível do otimizador 0, 1 ou 2 (omissão: 2; outro valor é erro) |
| `--color` / `--no-color` | todos | cores ANSI nos diagnósticos; omissão: só se o stderr for um terminal e `NO_COLOR` não estiver definido |
| `--include FILE`, `-I FILE`, `--include=FILE` (repetível) | check, run, compile, dump-*, optimize, verify, cfg, stats, profile, digest, bench | compila `FILE` junto com o ficheiro principal (ver abaixo) |
| `--max-steps N` | run, profile, digest, bench, benchmark, repl | orçamento de instruções da VM (omissão: 50 000 000) |
| `--max-depth N` | run, profile, digest, bench, benchmark, repl | profundidade máxima de chamadas (omissão: 10 000) |
| `--backend vm\|llvm` | run | `vm` (omissão) executa o bytecode; `llvm` emite LLVM IR e corre-o com `lli` |
| `--timings` / `--stats` | run | tempos por fase / valor, passos e relatório do otimizador (stderr) |
| `--unopt` | dump-ir | IR antes dos passes |
| `--emit`, `-o` / `--output` | compile | artefacto e ficheiro de saída |
| `--n N` | bench (omissão 5), benchmark (omissão 20) | repetições / argumento de `fib` |
| `--iters`, `--seed`, `--kind` | fuzz | configuração do fuzzer |

Opções mal formadas são erros (`exit 1`): nível `-O` fora de 0..2, valor em
falta (`--emit`, `-o`, `--n`, `--backend`...), `--max-steps 0`/`--max-depth 0`,
`--n 0` em `bench`, um segundo ficheiro operando (usar `--include`), um
ficheiro em `version`/`repl`/`benchmark`/`fuzz`, `--include=` vazio. Sem
comando, `aether` imprime a ajuda no stderr e sai com `1`; `aether help` e
`-h`/`--help` depois de qualquer comando imprimem-na no stdout com `0`.

Códigos de saída: `0` ok, `1` erro de compilação, uso errado da CLI ou falha
do `fuzz`, `2` erro de runtime (também em `benchmark`). Um stdout fechado
(`aether run f.ae | head -1`) termina em silêncio com `0`.
Num erro de runtime a CLI imprime o stdout produzido até ali antes de
`runtime error: ...`. Exceder `--max-steps` ou `--max-depth` é um erro de
runtime (`exit 2`) e o stdout parcial é preservado.

## Vários ficheiros: `use` e `--include`

Um ficheiro importa outro com `use "relative/path.ae";` (caminho relativo ao
ficheiro que importa; ver [`language.md`](language.md), “Módulos”). O
*driver* também aceita vários ficheiros na linha de comando: cada um recebe
o seu `FileId` na sessão e é lexado e analisado em separado (os diagnósticos
apontam para o ficheiro certo), as importações são seguidas
transitivamente, cada ficheiro entra uma vez (caminho canónico) e os itens
de todos são juntos num único programa. `--include` / `AETHER_INCLUDE` são
`use`s implícitos do ficheiro principal. Nomes repetidos entre ficheiros
dão o erro habitual `duplicate function`; uma importação que não existe, que
é um diretório ou que não é UTF-8 é `E0280 unresolved import` (exit 1).

### Visibilidade (`pub`)

O espaço de nomes é plano, mas um item definido num ficheiro **diferente** do
que o usa (via `use` ou `--include`) só é acessível se for `pub`; senão
`E0281` ("`NOME` is private to `FICHEIRO`", com a ajuda "mark it `pub` in
FICHEIRO"). Os itens do mesmo ficheiro são sempre acessíveis; os itens
privados do ficheiro principal não são acessíveis a ficheiros incluídos.
Aplica-se a funções, `extern fn`, `struct` e `enum` (o tipo, o literal
`S { .. }` / `E::A` e as anotações de tipo); os campos de uma `pub struct` e
as variantes de um `pub enum` são públicos. As importações são transitivas e
planas: `a` vê os itens `pub` de `b` e de tudo o que `b` importa. Os itens
privados pertencem ao seu ficheiro: dois ficheiros podem ter cada um a sua
`fn h()` privada sem colidir (cada um chama a sua); um item `pub` colide com
qualquer item do mesmo nome noutro ficheiro (`duplicate function`).

### Ficheiros e entradas estranhas

Um ficheiro vazio ou só com comentários dá `E0210` (falta `fn main`); um BOM
UTF-8 no início é ignorado; `\r\n` é aceite e as linhas dos diagnósticos
perdem o `\r`; as colunas contam caracteres (não bytes) e a linha de `^` copia
os tabs, por isso alinha com qualquer largura de tab. Ficheiros acima de
8 MiB (por ficheiro, incluindo `use` e `--include`), com bytes que não são
UTF-8 (`file is not valid UTF-8 (first bad byte at offset N)`),
diretórios e ficheiros em falta dão um erro `cannot read ...` numa linha.
Identificadores Unicode são `unexpected character` (não um *panic*).

### `fmt`

`fmt` escreve a AST do ficheiro principal (os itens importados não entram; os
`use` ficam como estavam). Erros léxicos/sintáticos terminam com `exit 1` sem
saída; erros de tipos não impedem a formatação. `aether fmt F > G; aether run G`
dá o mesmo que `run F`, e `fmt G` devolve `G` (idempotente): literais `f64`
ficam `3.0`, as anotações `let x: T` mantêm-se. Só saem os parênteses que a
gramática exige (e os de `(U {})` em cabeças de `if`/`while`/`match`/`for`),
`x op= e` mantém-se composto, cadeias `else if` e `if let` saem como foram
escritas e um retorno `unit` não ganha `-> ()`.

Os comentários (`// ...` e `/* ... */`, também os de várias linhas) são todos
preservados, uma vez cada, com o texto inalterado (só muda o fim de linha: o
`\r` de CRLF sai). Ficam presos ao elemento seguinte — item, instrução,
expressão final de um bloco, braço de `match`, campo de `struct` ou variante
de `enum` — numa linha própria antes dele; um comentário na mesma linha
depois de código (`let x = 1; // porquê`) ou depois de `{` fica nessa linha;
os que estão antes de `}` ficam dentro do bloco e os do fim do ficheiro ficam
no fim. Um comentário dentro de algo que não tem linha própria (expressão,
padrão, tipo, parâmetros, entre `}` e `else {`) passa para antes do elemento
que o contém. Linhas em branco entre elementos ou comentários são mantidas
(no máximo uma).

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
devolvido por `main` é o código de saída do processo (módulo 256): é o
resultado do programa, não uma falha, e `run` sai com `0` como na VM
(`--stats` imprime `exit = N`). Um erro de runtime (`abort()` depois de o
runtime escrever `runtime error: ...`, a mesma mensagem da VM) e um SIGSEGV
(pilha nativa esgotada) são erros de runtime (`exit 2`); um estado ≠ 0 acompanhado de mensagens do
`lli` no stderr (módulo rejeitado) é `exit 1`. `--max-steps`/`--max-depth`
não se aplicam a este motor: não há orçamento de instruções (o `--timeout`
pára um ciclo infinito) e a profundidade de chamadas tem o limite por
omissão da VM (10 mil frames contando `main`; `runtime error: call stack
overflow`, no mesmo ponto que a VM ao mesmo `-O`). O programa corre numa
thread com 1 GiB de pilha, para que recursão funda com agregados grandes
(que a VM guarda no heap) não esgote a pilha nativa. A API é `aether::driver::run_llvm_ir`, que
devolve stdout, stderr e o estado (`LliStatus::Exited(n)` / `Signaled(sig)`)
em separado. Contrato e limites: [`llvm.md`](llvm.md).

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
- Entradas que começam por `fn`, `struct`, `enum`, `extern`, `pub` ou `use`
  são **definições** (as que definem `fn main` correm como programa completo):
  ficam guardadas para as entradas seguintes (redefinir um nome substitui a
  versão anterior). Um erro numa definição nova descarta só essa entrada —
  as anteriores mantêm-se.
- Qualquer outra entrada é um bloco de instruções embrulhado num `main`
  novo e executado contra as definições guardadas; a CLI imprime o stdout e
  `=> valor`. Uma expressão final sem `;` (`dbl(3)`, `let x = 2; x * 3`)
  mostra o seu valor `i32`, ou é só avaliada pelos efeitos (`print_i32(5)`).
- Cada definição vive num pseudo-ficheiro próprio (`<repl:nome>`), mas no REPL
  todos os itens são públicos entre si: `E0281` não se aplica.
- Uma entrada com `fn main` corre como programa completo (nada é guardado).
- `:items` lista os nomes guardados, `:reset` esquece-os, `:help` ajuda,
  `:quit` (ou `:exit`, `:q`) sai; outro `:comando` é reportado como
  desconhecido. `-On` e `--max-steps`/`--max-depth` aplicam-se
  a cada execução.

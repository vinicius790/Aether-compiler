# Manual do repositório Aether 0.2.2

Este ficheiro explica **o que existe no ZIP**, **para que serve cada peça** e
**como usar, testar, estender e não partir** o compilador. Não substitui as
especificações em `docs/`; aponta para elas e descreve o fluxo de trabalho.

Índice

1. [O que é isto](#1-o-que-é-isto)
2. [O que o ZIP contém](#2-o-que-o-zip-contém)
3. [Como abrir, construir e verificar](#3-como-abrir-construir-e-verificar)
4. [A linguagem em dez minutos](#4-a-linguagem-em-dez-minutos)
5. [A cadeia de compilação](#5-a-cadeia-de-compilação)
6. [A CLI, comando a comando](#6-a-cli-comando-a-comando)
7. [Otimizador](#7-otimizador)
8. [Máquina virtual e LLVM](#8-máquina-virtual-e-llvm)
9. [Fuzzing](#9-fuzzing)
10. [Ferramentas extra (`fmt`, `cfg`, `verify`)](#10-ferramentas-extra-fmt-cfg-verify)
11. [Mapa de ficheiros](#11-mapa-de-ficheiros)
12. [Como acrescentar uma funcionalidade](#12-como-acrescentar-uma-funcionalidade)
13. [Como reportar um bug](#13-como-reportar-um-bug)
14. [Limitações honestas](#14-limitações-honestas)
15. [Referência cruzada da documentação](#15-referência-cruzada-da-documentação)

---

## 1. O que é isto

Aether é uma linguagem pequena, **estaticamente tipada** e **imperativa**,
acompanhada de um compilador escrito em Rust 1.75 (edição 2021) **sem
dependências crates.io**.

O objectivo do repositório não é “mais um interpretador de árvore”. É uma
cadeia real:

```
fonte .ae
  → lexer
  → parser
  → AST
  → análise semântica / tipos (HIR)
  → IR de três endereços + blocos básicos
  → otimizador
  → backend
       ├─ bytecode de registradores → VM própria   (contrato de execução)
       └─ LLVM IR textual                          (inspeção / experiência)
```

A VM é o backend em que se confia. O LLVM emitido é texto; não há JIT
ligado, não há ABI estável, não há `llc` no caminho crítico.

Versão do software e da linguagem neste ZIP: **0.2.2** (ver `CHANGELOG.md`).
Licença: MIT (`LICENSE`, `NOTICE`).

---

## 2. O que o ZIP contém

Descompactar:

```bash
unzip aether-compiler.zip
cd aether
```

Árvore relevante (sem `.git/`):

| Caminho | Função |
|---------|--------|
| `README.md` | Porta de entrada (inglês) |
| `MANUAL.md` | Este manual |
| `Cargo.toml` / `Cargo.lock` | Pacote Rust `aether` 0.2.2, binário + lib |
| `rust-toolchain.toml` | MSRV 1.75.0 + rustfmt/clippy |
| `rustfmt.toml` / `clippy.toml` / `.editorconfig` | Estilo |
| `Makefile` | Atalhos `test`, `examples`, `fuzz`, `verify`, `release` |
| `src/` | Compilador, VM, fuzzer, CLI |
| `examples/` | Programas `.ae` usados também como testes |
| `stdlib/math.ae` | Rotinas de referência (`abs`, `min`, `max`, `clamp`) |
| `benchmarks/` | Programas para `aether benchmark` / medições manuais |
| `docs/` | Especificação e notas de desenho |
| `tests/` | Testes de integração contra o binário |
| `scripts/` | `fuzz.sh`, `count_lines.sh` |
| `.github/` | CI, templates de issue/PR, CODEOWNERS |
| `LICENSE` `NOTICE` `CHANGELOG.md` `CITATION.cff` `AUTHORS` | Legal / citação |
| `SECURITY.md` `CODE_OF_CONDUCT.md` `CONTRIBUTING.md` `GOVERNANCE.md` `SUPPORT.md` | Governação |

Não há `target/` no ZIP. É preciso compilar.

---

## 3. Como abrir, construir e verificar

### Requisitos

- Rust **1.75 ou superior**. O ficheiro `rust-toolchain.toml` pede exactamente
  `1.75.0`. Se `rustup` estiver instalado, entra sozinho no directório.
- Sistema tipo Unix (o CI é Ubuntu). Windows funciona se o Rust estiver
  instalado; os scripts em `scripts/` e o `Makefile` são POSIX.
- LLVM 18 é **opcional**. Só precisa dele se quiser passar o texto de
  `aether dump-llvm` a `opt` / `lli`. A execução normal **não** usa LLVM.

### Construir

```bash
cargo build --release
# binário: target/release/aether
```

Ou:

```bash
make release
```

### Testar (obrigatório antes de declarar o ZIP “ok”)

```bash
cargo test
make examples
```

O que `cargo test` cobre:

- testes unitários em `src/**` (lexer, parser, sema, IR, opt, VM, fuzz, pretty, cfg, verify)
- `tests/cli_examples.rs` — o binário corre `hello`, `opt_demo`, `check`
- `tests/tools.rs` — `verify`, `cfg`, `fmt`, `opaque.ae` `-O2`, `stdlib/math.ae`
- `tests/regressions.rs` — um teste por bug corrigido (R1–R22 em
  `docs/test-matrix.md`); cada programa corre em `-O0` e `-O2`, que têm de
  concordar e coincidir com o stdout/valor esperado

Ordem de magnitude no estado 0.2.2: dezenas de testes, todos no mesmo
processo `cargo test`. Se algum falhar, o repositório **não** está utilizável;
não ignore e não “ajuste o assert”.

### Primeiro programa

```bash
cargo run -- run examples/hello.ae
cargo run -- run examples/fib.ae -O2 --stats --timings
cargo run -- run examples/opaque.ae -O2
cargo run -- run stdlib/math.ae
```

`stdlib/math.ae` **não** é importado por outros ficheiros. A linguagem 0.2
não tem `mod` / `use`. O ficheiro é um programa autónomo com `main`, para
copiar rotinas para o seu `.ae`.

---

## 4. A linguagem em dez minutos

Especificação completa: `docs/language.md`. Gramática EBNF: `docs/grammar.md`.

### Ficheiro

Extensão habitual: `.ae`. Um programa executável precisa de `fn main`.
`main` deve devolver `i32` nos exemplos oficiais (código de “resultado”,
não necessariamente código de saída do processo — a CLI imprime o valor).

### Tipos

| Tipo | Notas |
|------|--------|
| `i32` `i64` | inteiros com wrap (soma, produto e divisão) |
| `f64` | IEEE |
| `bool` | `true` / `false` |
| `string` | concatenação com `+` no fold de constantes |
| `char` | |
| `unit` / `()` | |
| `[T; N]` | arrays de tamanho fixo |
| `struct Nome { campo: T, ... }` | |

Não há referências, traits, generics, nem heap exposto ao utilizador.

Regras que costumam surpreender (detalhe em `docs/language.md`):

- A expressão final sem `;` do corpo de uma função é o valor devolvido e
  tem de ter o tipo de retorno (E0221). Em blocos aninhados é avaliada
  como instrução.
- Arrays e structs têm **semântica de valor**: `let b = a;` copia.
- `&&` / `||` avaliam o operando direito só quando é preciso.
- Literais negativos são literais únicos; `-2147483648` é um `i32` válido.
  Um literal inteiro fora do intervalo de `i32` é erro (E0263) e um literal
  nu adopta o tipo do outro operando (`1 + a` com `a: i64` é `i64`).
- Funções não são valores: usar o nome de uma função como valor é E0264.
- Os limites de `for` são `i32` (E0238); a variável só existe no corpo.
- `len` conta caracteres (valores escalares Unicode), como a indexação.
- Aninhamento acima de 512 níveis (expressões ou blocos) é diagnosticado
  (E0101).

### Controlo

`if` / `else`, `while`, `for i in a..b`, `return`, `break`, `continue`.
`let` e `let mut`. Inferência a partir do inicializador.

### Built-ins

Declarados no semântico e implementados **duas vezes**: VM
(`src/vm/mod.rs`) e wrappers LLVM (`src/backend/llvm.rs`). Lista canónica
em `src/runtime/mod.rs`:

`print`, `println`, `print_i32`, `print_i64`, `print_f64`, `print_bool`,
`len`, `assert`.

Acrescentar um built-in implica tocar **os quatro sítios** (sema, tabela
de natives no bytecode, `call_native` na VM, opcionalmente LLVM) e o
comentário em `src/runtime/mod.rs`.

### Exemplo mínimo

```
fn main() -> i32 {
    print_i32(40 + 2);
    return 0;
}
```

---

## 5. A cadeia de compilação

Orquestração: `src/driver.rs`. API de biblioteca: `src/lib.rs`
(`compile_source`, `run_source`).

### 5.1 Lexer — `src/lexer.rs`

Consome o texto, produz `Token` (`src/token.rs`) com `Span` (`src/span.rs`).
Comentários `//` e `/* */` não aninhados. Literais inteiros cabem em `i64`
na análise léxica.

**Como inspeccionar**

```bash
aether dump-tokens examples/hello.ae
```

### 5.2 Parser — `src/parser.rs`

Descida recursiva + precedência de operadores. Produz `Program` em
`src/ast.rs`. Erros vão para `src/diagnostic.rs` (não aborta no primeiro
token se conseguir recuperar).

```bash
aether dump-ast examples/structs.ae
```

### 5.3 Semântica — `src/sema/`

Tabela de símbolos com scopes (`sema/scope.rs`). Verificação de tipos
(`src/ty.rs`). Resultado: HIR usado pelo lowering.

```bash
aether check examples/hello.ae
```

Um programa mal tipado deve sair com código 1 e diagnóstico no stderr.

### 5.4 IR — `src/ir/`

IR **não-SSA**, três endereços, blocos básicos, terminadores
`Jump` / `Branch` / `Return` / `Unreachable`.

- `mod.rs` — tipos e `emit_ir`
- `dump.rs` — texto
- `verify.rs` — invariantes (ids de bloco, intervalo de registradores, `main`)
- `cfg.rs` — Graphviz DOT

```bash
aether dump-ir examples/opt_demo.ae --unopt
aether dump-ir examples/opt_demo.ae -O2
aether verify examples/fib.ae
aether cfg examples/fib.ae > /tmp/fib.dot
```

A IR **não é SSA**. Um pass que propaga um valor através de um join sem
φ-nodes está errado. O otimizador e o gerador MIR do fuzzer aprenderam
isto da maneira difícil: um `acc` reescrito noutro bloco “morto” mudava
o resultado entre `-O0` e `-O2`.

### 5.5 Otimizador — `src/opt/mod.rs`

Ver secção 7.

### 5.6 Backends — `src/backend/`

- `bytecode.rs` — assembler da VM
- `llvm.rs` — emissor de texto LLVM

```bash
aether dump-bytecode examples/loops.ae
aether dump-llvm examples/hello.ae
aether compile examples/hello.ae --emit ir -o /tmp/hello.ir
```

### 5.7 Execução — `src/vm/mod.rs`

Máquina de registradores, limite de passos (anti-loop no fuzzer),
cobertura de arestas opcional para greybox.

---

## 6. A CLI, comando a comando

Binário: `src/main.rs`. **Não** há `clap`. Os argumentos são lidos à mão.

Forma geral:

```
aether <comando> [opções] [ficheiro]
```

Opções comuns:

| Opção | Efeito |
|-------|--------|
| `-O0` `-O1` `-O2` ou `-O n` | nível do otimizador (omissão: 2) |
| `--unopt` | dump da IR antes dos passes |
| `--timings` | tempos por fase |
| `--stats` | estatísticas |
| `--emit ir\|bytecode\|llvm` | artefacto de `compile` |
| `-o PATH` | ficheiro de saída |
| `--iters N` | iterações do fuzzer (omissão: 200) |
| `--seed N` | semente (aceita `0x…`) |
| `--kind …` | suíte de fuzz |
| `--n N` | argumento de `benchmark` (Fibonacci) |

Códigos de saída: `0` ok, `1` erro de compilação / fuzz failure, `2` erro de
runtime. Num erro de runtime a CLI imprime primeiro o stdout produzido até
ali e depois `runtime error: ...`.

### `check`

Só semântica. Não corre a VM.

### `run`

Compila e executa. É o comando do dia-a-dia.

```bash
aether run examples/opt_demo.ae -O2 --timings --stats
```

### `compile`

Escreve IR / bytecode / LLVM num ficheiro.

### `dump-*` e `disassemble`

Inspeção. `disassemble` é alias de `dump-bytecode`. Além de `dump-tokens`,
`dump-ast`, `dump-ir`, `dump-bytecode` e `dump-llvm`:

- `dump-hir <file>` — HIR tipada
- `dump-liveness <file> [-On]` — conjuntos live-in por bloco

### `stats` `profile` `digest`

- `stats <file> [-On]` — relatório do otimizador + liveness + tamanho da IR
- `profile <file> [-On]` — contagem de chamadas por função + digest da execução
- `digest <file> [-On]` — impressão digital determinística de stdout + valor
  (FNV-1a de `"{value}\n{stdout}"`); duas compilações do mesmo fonte (`-O0` vs
  `-O2`) têm de dar o mesmo digest

### `optimize`

Imprime o relatório **instruções antes → depois** por pass. É a prova
mensurável de que o otimizador faz alguma coisa (não inventar speedups).

### `repl`

Linhas até uma linha vazia = um programa. Se não houver `fn main`, o
texto é envolvido num `main`. `:quit` sai.

### `benchmark`

Compila `fib(n)` em `-O0` e `-O2`, corre os dois, imprime valor, passos
da VM e microsegundos. Recursão de Fibonacci **quase não muda de forma**;
o ganho aparece em constante / DCE (`examples/opt_demo.ae`).

### `fuzz`

Secção 9.

### `fmt` `cfg` `verify`

Secção 10.

### `help` / `version`

Meta.

---

## 7. Otimizador

Documento: `docs/optimizations.md`. Código: `src/opt/mod.rs`.

Níveis:

- `-O0` — não corre passes (a IR baixa-se na mesma)
- `-O1` — fold, copy-prop, DCE (subconjunto)
- `-O2` — sequência completa, duas varridas de fold/DCE

Passes em `-O2`, pela ordem em que correm:

1. `const-fold` — `Bin`/`Un` com ambos os lados constantes (inclui
   comparações `i64`/`f64`/`char`/`string`/`bool` e `i64` `%`; a divisão
   usa wrapping); `br` constante → `jmp`
2. `algebraic` — `x+0`, `x-0`, `x*1`, `x/1`, `x*0`, `x*2 → x+x`, **predicados opacos**
3. `cf-simplify` — blocos vazios, inatingíveis (antes do inline, para que
   as folhas de um só bloco sejam reconhecidas: o lowering deixa um bloco
   morto depois de cada `return`)
4. `inline` — copia folhas de um bloco para o chamador; os argumentos vão
   para registradores novos e todos os tipos de instrução são remapeados
5. `local-cse` — value numbering dentro do bloco
6. `copy-prop` — substitui usos de `Move`; não redirecciona a base de um
   store de elemento/campo e descarta aliases dos dois lados de um store
   (semântica de valor)
7. `const-prop` — segunda varrida de fold
8. `cf-simplify` — outra vez, depois do inline
9. `dce` — instruções puras cujo destino nunca é lido
10. `const-fold` e `dce` outra vez

Identidades opacas, **mesmo registrador**, sem exigir constante. Aplicam-se
só a `i32` e `i64`; **nunca** a `f64` (com NaN, `x - x`, `x * 0` e `x == x`
não dão `0` / `true`):

| Padrão | Resultado |
|--------|-----------|
| `x - x` | `0` |
| `x * 0` / `0 * x` | `0` |
| `x == x` | `true` |
| `x != x`, `x < x`, `x > x` | `false` |
| `x <= x`, `x >= x` | `true` |

Isto é o que mata os *decoy blocks* gerados pelo fuzzer MIR
(`acc-acc==0`, `acc*0==0`) depois de `-O2`. Demonstração:

```bash
aether dump-ir examples/opaque.ae --unopt
aether dump-ir examples/opaque.ae -O2
aether run examples/opaque.ae -O2
```

Esperado: valor `1`; no dump `-O2` o `br` do predicado deve ter
desaparecido (vira `jmp`).

Regra de ouro: **não** propague um registrador através de um join.
Há um teste `does_not_fold_across_reassignment` exactamente para isso.

---

## 8. Máquina virtual e LLVM

### VM — `src/vm/mod.rs`, `docs/vm.md`

- Registradores, não stack de operandos como JVM.
- Natives via `CallNative`. Chamar uma `extern fn` que a VM não implementa
  é erro de runtime.
- Registradores `u16`: até 65535 por frame. O assembler devolve erro de
  compilação em vez de truncar.
- Comparações e conversões que a sema aceita têm todas opcode: `Cmp { op }`
  genérico (`Eq Ne Lt Le Gt Ge`) para `i64`/`f64`/`bool`/`char`/`string`,
  `RemI64`, `NegI64` e as conversões `CastI64ToF64`, `CastF64ToI64`,
  `CastBoolToI64`, `CastCharToI32`, `CastI32ToChar`. Uma combinação
  (operador, tipo) sem opcode é erro de compilação (E0300), nunca um `Nop`.
- Divisão inteira com wrapping (`i32::MIN / -1 == i32::MIN`); divisão por
  zero continua a ser erro de runtime.
- Arrays e structs copiam-se por valor (cópia profunda).
- `VmOptions { coverage }` activa o mapa de arestas `(func, pc_prev) → (func, pc)`
  usado pelo greybox.
- Limite de passos: um programa gerado pelo fuzzer não pode pendurar o
  processo.

Contrato: dois compilados do mesmo fonte, `-O0` e `-O2`, têm de devolver
o **mesmo** `Value` e o **mesmo** stdout. Isto é a propriedade
`differential`.

### LLVM — `src/backend/llvm.rs`

Texto. Serve para `aether dump-llvm` e `--emit llvm`. Produz LLVM 18 IR
válido: um `alloca` por registrador da IR, `load`/`store`, pronto para
`mem2reg` (verificado com `llvm-as` em todos os exemplos). Diferenças
conhecidas face à VM, que continua a ser o contrato de execução: agregados
vivem na stack e copiar um array/struct copia o ponteiro (há aliasing); sem
verificação de limites; concatenação de strings não é suportada (aborta);
`print_f64` usa `%g`. Não há ligação a `inkwell` / `llvm-sys`. Se `opt-18` estiver no PATH, pode-se fazer
experimentos **fora** do repositório; o CI não depende disso.

---

## 9. Fuzzing

Documentos: `docs/fuzzing.md`, `docs/greybox.md`, `docs/aspect-mir.md`,
`docs/fuzzers-rust.md`. Código: `src/fuzz/`.

Não há `libfuzzer`, `cargo-fuzz`, `proptest` nem `arbitrary`. O gerador
é um xorshift64* determinístico (`src/fuzz/rng.rs`). Uma falha imprime
**semente + caso + fonte**. Replay:

```bash
aether fuzz --kind diff --seed 0xDEAD --iters 1
```

### Kinds

| `--kind` | Aliases | O que faz |
|----------|---------|-----------|
| `all` | | corre as propriedades “baratas” em sequência |
| `lexer` | | bytes / texto aleatório → lexer não pânica; tokens bem formados |
| `parser` | | idem para o parser |
| `pipeline` | | `compile_source` não pânica |
| `generated` | `gen` | programa da gramática **compila** |
| `differential` | `diff` | `-O0` e `-O2` concordam em valor e stdout |
| `agg` | `aggregate` | programas bem tipados com structs, arrays (também aninhados), `i64`/`f64`/`char`/`string`, conversões, guardas `&&`/`\|\|`, atribuição aninhada e funções com expressão final; oráculo diferencial O0/O2 |
| `mutated` | `mut` | havoc sobre fonte válida, sem pânico |
| `structural` | `struct` | mutação da *sketch* (árvore) |
| `aspect` | `ap` | mutação que preserva tipos / nomes / terminação |
| `mir` | `ir` | gera IR directa com decoys; verifica + corre O0/O2 |
| `greybox` | `grey` `graybox` | corpus + energia + cobertura de arestas da VM |
| `format` | `fmt` `grammar` | fita de bytes = escolhas da EBNF |

`all` inclui `agg` e **não** inclui greybox (campanha com estado) nem
format; esses chamam-se à parte. As propriedades de lixo, format e
mutação compilam também em `-O2`.

### Como correr

```bash
aether fuzz --iters 200 --kind all --seed 1
aether fuzz --iters 80 --kind format
aether fuzz --iters 40 --kind greybox
aether fuzz --iters 200 --kind diff
aether fuzz --iters 100 --kind mir
aether fuzz --iters 200 --kind agg
```

Ou `make fuzz` (40 iters, `all`, seed 1).

### O que cada módulo é

- `sketch.rs` — programa como árvore tipada (`STy::I32` / `Bool`), helpers + `main`
- `mutate.rs` — mutação estrutural, crossover, shrink
- `mir.rs` — `FunBuilder`, decoys (const true/false, opacos, cadeias)
- `greybox.rs` — corpus de `IrModule`, schedule de energia
- `format.rs` — decoder de bytes → fonte quase gramatical
- `gen.rs` — síntese clássica de fonte bem tipada
- `mod.rs` — orquestração, `FuzzKind`, propriedades, relatório

### Relação com ferramentas Java / Rust do ecossistema

Isto está escrito em `docs/fuzzers-rust.md`. Resumo:

- `mut` ≈ Jazzer `byte[]` / AFL havoc
- `format` ≈ Grammarinator / Nautilus (sem ANTLR)
- `aspect` ≈ Zest (JQF)
- `greybox` ≈ Jazzer / libFuzzer sem o motor C++
- `diff` é o oráculo de compilador (não existe “de fábrica” no Jazzer)

Não adicione `libafl` sem actualizar `docs/status.md`. A decisão do
projecto é MSRV 1.75 e zero crates.

---

## 10. Ferramentas extra (`fmt`, `cfg`, `verify`)

Documento curto: `docs/tools.md`.

### `aether fmt ficheiro.ae`

Pretty-print a partir da AST (`src/pretty.rs`). Não é `rustfmt` da
linguagem Aether: é um pretty-printer honesto do que o parser viu.
Útil para ver como a árvore ficou, não para “formatar o projecto”.

### `aether cfg ficheiro.ae [-On]`

DOT do CFG da IR. Visualizar:

```bash
aether cfg examples/fib.ae -O0 > /tmp/fib.dot
dot -Tpng /tmp/fib.dot -o /tmp/fib.png   # se Graphviz estiver instalado
```

### `aether verify ficheiro.ae [-On]`

Corre `ir::verify::verify_module`: existe `main`, blocos com ids únicos,
terminadores apontam para blocos que existem, registradores < `reg_count`,
nenhum `Unreachable` residual. O fuzzer MIR usa a **mesma** função
(`fuzz::mir::check_ir` delega).

---

## 11. Mapa de ficheiros

### `src/` — o compilador

| Ficheiro | Responsabilidade |
|----------|------------------|
| `lib.rs` | API pública, `run_source`, testes e2e |
| `main.rs` | CLI |
| `driver.rs` | pipeline e `Compiled` |
| `lexer.rs` `token.rs` `span.rs` | léxico |
| `parser.rs` `ast.rs` `pretty.rs` | sintaxe |
| `diagnostic.rs` | erros com span |
| `ty.rs` | tipos |
| `sema/mod.rs` `sema/scope.rs` | nomes e tipos |
| `ir/mod.rs` `ir/dump.rs` `ir/verify.rs` `ir/cfg.rs` | IR |
| `opt/mod.rs` | passes |
| `backend/bytecode.rs` `backend/llvm.rs` | backends |
| `vm/mod.rs` | execução |
| `runtime/mod.rs` | lista de natives |
| `fuzz/*` | geradores e propriedades |

### Exemplos

| Ficheiro | Para quê |
|----------|----------|
| `examples/hello.ae` | mínimo |
| `examples/fib.ae` | recursão |
| `examples/loops.ae` | `while` / `for` |
| `examples/arrays.ae` | `[T; N]` |
| `examples/structs.ae` | structs |
| `examples/opt_demo.ae` | constantes + DCE mensurável |
| `examples/opaque.ae` | predicados opacos |

### Governação (inglês de propósito)

O README e os ficheiros de política estão em inglês para o repositório
poder ser hospedado e citado. Várias notas em `docs/` estão em
português porque a especificação original foi escrita assim. Não
traduza à força num PR misturado com código.

---

## 12. Como acrescentar uma funcionalidade

Leia `CONTRIBUTING.md` primeiro. A regra é uma só: **a mesma PR traz
sintaxe, semântica, IR, execução, teste e uma linha em `docs/status.md`**.

### Nova construção da linguagem (exemplo: `match`)

1. Gramática em `docs/grammar.md` e parágrafo em `docs/language.md`.
2. Token se for palavra-chave (`src/token.rs`, lexer).
3. Nós em `src/ast.rs`, parse em `src/parser.rs`, pretty em `src/pretty.rs`.
4. Tipos em `sema`.
5. Lowering em `ir` (novos blocos, terminadores existentes se chegar).
6. Se precisar de opcode novo: `bytecode.rs` + `vm`.
7. Teste unitário + um `examples/…ae`.
8. Se a construção entrar no gerador, actualizar `fuzz/gen.rs` e/ou `sketch.rs`.

### Novo pass do otimizador

1. Função `pass_*` em `src/opt/mod.rs` (ou módulo novo, reexportado).
2. Meter na lista de `-O2` **depois** de justificar a ordem.
3. Teste “antes contém X, depois contém Y” **e** teste de não-regressão
   (`does_not_fold_across_reassignment` é o modelo).
4. Correr `aether fuzz --kind diff --iters 500`.

### Novo built-in

Sema + `runtime::NATIVES` + id no assembler + `call_native` + LLVM
wrapper se o dump LLVM tiver de o conhecer.

### Novo kind de fuzz

1. Variante em `FuzzKind`.
2. Parse do CLI (`parse` + `kind_salt` + `match` em `run_fuzz`).
3. Propriedade `prop_*`.
4. Linha em `docs/fuzzing.md` e neste manual.
5. Teste curto em `fuzz::tests`.

### O que **não** fazer

- Adicionar `serde`, `anyhow`, `clap`, `inkwell`, `libafl` “porque é
  profissional”. O profissionalismo deste repo é **zero crates** e MSRV
  fixo.
- Gerar 80 mil linhas de boilerplate.
- Inventar um segundo frontend.
- Tratar o LLVM textual como ABI.

---

## 13. Como reportar um bug

`SECURITY.md` e `.github/ISSUE_TEMPLATE/bug.yml`.

Um relatório útil tem:

1. Versão (`0.2.2`).
2. Fonte reduzida (`.ae`).
3. Comando exacto.
4. Esperado vs observado.
5. Se veio do fuzzer: `--kind` e `--seed`.
6. Se é miscompile: `aether dump-ir --unopt` e `aether dump-ir -O2`.

Não abra issue de “faltam generics” sem RFC (`ISSUE_TEMPLATE/feature.yml`).

---

## 14. Limitações honestas

Isto **não** é o LLVM, **não** é o rustc, **não** é um produto com SLA
(`SUPPORT.md`). Em 0.2.2:

- sem módulos / `use` / pacotes
- sem SSA, sem alocação de registradores global; o inlining só cobre
  folhas de um bloco
- LLVM é texto, sem JIT/AOT no CI; os agregados fazem aliasing e não há
  verificação de limites nem concatenação de strings (a VM é o contrato)
- bytecode instável entre versões (`README` / `CHANGELOG`)
- stdlib é um ficheiro de referência, não uma biblioteca ligada
- o pretty-printer não é um formatador de projecto
- o greybox é in-process e pequeno; não substitui OSS-Fuzz
- a linguagem cabe na cabeça; a profundidade está na integração das camadas

O número de linhas do `src/` está na ordem das **dezenas de milhar no
máximo baixo**, não 80k. Contar: `bash scripts/count_lines.sh`. Qualidade
≠ volume.

---

## 15. Referência cruzada da documentação

| Precisa de… | Abra |
|-------------|------|
| Sintaxe e semântica | `docs/language.md` |
| EBNF | `docs/grammar.md` |
| Diagrama do pipeline | `docs/architecture.md` |
| Forma da IR | `docs/ir.md` |
| Passes | `docs/optimizations.md` |
| VM | `docs/vm.md` |
| Comandos | `docs/cli.md`, `docs/tools.md` |
| Fuzzer | `docs/fuzzing.md`, `docs/greybox.md`, `docs/aspect-mir.md` |
| Comparação com cargo-fuzz / Jazzer | `docs/fuzzers-rust.md` |
| O que está feito / em falta | `docs/status.md` |
| Pasta Compiladores do Drive | `docs/alexandria.md` |
| Índice dos docs | `docs/README.md` |
| Como contribuir | `CONTRIBUTING.md` |
| Segurança | `SECURITY.md` |
| Histórico de versões | `CHANGELOG.md` |
| Como citar | `CITATION.cff` |

---

Fim do manual. Se este ficheiro e o código divergirem, o código + o teste
ganham; actualize o manual na mesma PR.

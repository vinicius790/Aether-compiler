# Mutação aspect-preserving e geração directa de IR

Duas técnicas de fuzzing de compiladores que o Aether passa a
implementar no próprio repositório.

## Aspect-preserving mutation (DIE)

DIE (Park et al., *Fuzzing JavaScript Engines with Aspect-preserving
Mutation*, S&P 2020) observa que um semente que *já* passou o
typechecker é um bem raro. Mutar bytes — ou mesmo a árvore sem olhar
para tipos — deita esse bem fora: a maior parte dos mutantes morre
outra vez no frontend.

Um *aspecto* é uma propriedade da semente que queremos conservar com
alta probabilidade:

| Aspecto | No Aether | Como se conserva |
|---------|-----------|------------------|
| tipo | cada `Expr` tem `STy` | reescrita só para um nó do mesmo `STy` |
| nome | ligações vivas | não se apaga um `let` ainda referido; não se escreve no índice do `for` |
| terminação | `for lo..hi` com `hi-lo ≤ 3` | o mutador só mexe nos bounds dentro dessa janela |
| aridade | helpers `i32 → i32` | crossover só entre helpers com a mesma assinatura |
| valor (subconjunto) | `main` e o stdout | só identidades: `e+0`, `if true {…}`, `let` morto |

A sketch tipada vive em `src/fuzz/sketch.rs`. O texto-fonte é um
*pretty-printer*. Mutar a sketch e só depois `render()` é o que
distingue `--kind aspect` de `--kind struct` (este último ainda opera
em linhas).

```
semente bem tipada
        │
        ▼
 mutate_aspect(preserve_value?)
        │
        ▼
 render → compile_source
        │
        ├─ tem de tipar sempre          (aspecto tipo)
        └─ se preserve_value:
              O0 == O2                  (oráculo diferencial)
```

Isto é o homólogo local de `fuzz_mutator!` + mutatis, sem a crate
(MSRV). A diferença para o havoc (`--kind mut`) é deliberada: havoc
existe para o lexer; aspect existe para o meio do compilador.

## Geração directa de IR (Rustlantis)

Rustlantis (OOPSLA 2024) não gera Rust. Gera **MIR**, a IR sobre a
qual o rustc optimiza, porque satisfazer o borrow checker a partir da
fonte é um desperdício quando o alvo é o optimizador.

Aether não tem borrow checker, mas o argumento aguenta-se:

* o lexer/parser/sema já estão cobertos pelo gerador de fonte;
* bugs de opt/codegen querem CFGs *feios* (ramos mortos, identidades
  `x+0`, chamadas, laços de trip-count conhecido);
* gerar `IrModule` salta três etapas e deixa o oráculo diferencial
  correr em cima do mesmo tipo que o optimizador reescreve.

`src/fuzz/mir.rs` constrói:

* um `main: () → i32` e zero ou um helper `hN: i32 → i32`;
* blocos com terminadores válidos e registradores `< reg_count`;
* **decoy blocks** em cinco sabores — literal `true` / `false`,
  predicados opacos `(acc-acc)==0` e `(acc*0)==0`, cadeia `d1→d2→real`.
  `-O0` toma só o lado vivo; `-O2` deve dobrar o que for constante.
  O lado morto nunca escreve o acumulador (IR não-SSA);
* um “laço” de trip-count constante `0..n` (`n ≤ 3`) que o const-fold
  deve linearizar;
* `print_i32` como `Call` nativo, para o oráculo de stdout.

`--kind mir` verifica:

1. `check_ir` antes e depois de `-O2` (o pass não pode deixar
   `Unreachable` nem um jump para um bloco que não existe);
2. o optimizador e a VM não fazem panic;
3. `-O0` e `-O2` concordam no valor de `main` e no stdout.

Não é LLVM MIR nem rustc MIR. É a IR de três endereços do Aether.
O passo seguinte, quando o toolchain subir de 1.75, é
`#[derive(Arbitrary)]` sobre `Inst` / `Terminator` e um target
`cargo-fuzz` — a mesma tese do Rust Fuzz Book: estrutura na IR, não
no fonte.

## O que isto *não* é

* `--kind mir` não é coverage-guided. O loop greybox da VM está em
  [`docs/greybox.md`](greybox.md) (`--kind greybox`).
* Não obscurece valores à la Rustlantis (ainda). Um `LoadConst true`
  no decoy é *propositalmente* visível para o const-fold: queremos
  testar o pass, não escondê-lo.
* Não gera SSA. A IR do Aether é de registradores estáveis; um gerador
  SSA seria um projecto diferente.

## Comandos

```bash
aether fuzz --kind aspect --iters 200 --seed 1
aether fuzz --kind mir    --iters 200 --seed 1
cargo test aspect_mutants_typecheck mir_programs_agree -- --nocapture
```

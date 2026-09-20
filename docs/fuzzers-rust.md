# Fuzzers de propriedade em Rust e mutação estrutural

Pesquisa de 2026 sobre o que existe no ecossistema e o que o Aether
adopta *dentro da árvore* (sem crates externas: o toolchain do CI é
1.75 e várias crates recentes pedem edition 2024).

## Dois eixos que as pessoas misturam

| Eixo | Pergunta | Exemplos |
|------|----------|----------|
| **Geração vs mutação** | O caso nasce do nada ou a partir de um corpus? | Csmith / gerador Aether vs AFL / Superion |
| **Estrutura vs bytes** | A transformação conhece a gramática? | `Arbitrary` / AST splice vs bit-flip |
| **Cobertura vs propriedade** | O oráculo é “não crashou” ou “P(x) vale”? | libFuzzer vs `proptest` |
| **Greybox vs blackbox** | Há feedback de ramos? | cargo-fuzz vs campanha `aether fuzz` |

Um *fuzzer de propriedade* combina o terceiro eixo com qualquer um dos
outros: gera/muta entradas e verifica um predicado (`O0 == O2`,
`tokens.last() == Eof`, …), não só a ausência de SIGSEGV.

## Ecossistema Rust (2025–2026)

### Property testing (blackbox, CI)

| Crate | Ideia | Shrinking | Notas |
|-------|-------|-----------|-------|
| **proptest** | *Strategies* composáveis | Forte (árvore de escolhas) | O mais usado. Seeds gravadas em `proptest-regressions/`. |
| **quickcheck** | `Arbitrary` próprio | Fraco em tipos do utilizador | API curta; alto *throughput*. |
| **arbtest** | Reusa `arbitrary::Arbitrary` | Via o mesmo trait | Uma API para `cargo test` e `cargo fuzz`. |
| **bolero** | Front-end único | Engine-dependente | `cargo bolero test` no CI; `cargo bolero fuzz` com libFuzzer. |
| **mutatis** (`check`) | Mutadores estruturais | Por construção | MSRV 1.91 — fora do nosso 1.75. |
| **pbt-macro** | Casos de canto + BFS no AST do tipo | Exhaustivo no início | edition 2024. |
| **derive_fuzztest** (Google) | Um `#[fuzztest]` → *quickcheck* + *cargo-fuzz* | O do backend | Boa ponte CI ↔ campanha longa. |

Padrão comum: *generate → run property → shrink → record seed*.

### Greybox (cobertura, campanha longa)

| Ferramenta | Engine | Precisa nightly? | Estrutura |
|------------|--------|------------------|-----------|
| **cargo-fuzz** + libFuzzer | LLVM SanitizerCoverage | Sim | `Arbitrary` e/ou `fuzz_mutator!` |
| **cargo-afl** / afl.rs | AFL++ | Não (stable) | custom mutator + CmpLog |
| **honggfuzz-rs** | Honggfuzz | Stable | `arbitrary` opcional |
| **LibAFL** | Biblioteca, não CLI | Stable | Combina engines; mais código |
| **cargo-libafl** | LibAFL via cargo | — | Menos maduro que cargo-fuzz |

Experiência documentada por Fitzgerald (2026-06): para inputs
estruturados, **mutação consciente da estrutura cobre mais, ao longo
do tempo, do que só `Arbitrary` a gerar do zero**. Geração e mutação
não se excluem.

`libfuzzer-sys` 0.4.13 aponta explicitamente para **mutatis** como o
sítio onde escrever `fuzz_mutator!`.

### Compiladores como alvo

| Sistema | O que gera | Oráculo |
|---------|------------|---------|
| **Csmith** | C bem definido, sem UB | diferencial GCC/Clang/`-O0`/`-O2` |
| **YARPGen** | C++, stress em opts | idem |
| **Rustlantis** | MIR, não Rust fonte | diferencial rustc |
| **tree-crasher / tree-splicer** | splice de subárvores tree-sitter | ICE do rustc |
| **ClozeMaster** | máscara + LLM em bugs históricos | ICE / rejeição |
| **MetaMut** | mutadores semânticos gerados por LLM | bugs GCC/Clang |

Para um compilador, bytes aleatórios morrem no lexer. O valor está em
programas que *passam* o frontend e exercitam IR / opt / codegen.

## Técnicas de mutação estrutural

Operadores que a literatura (Superion, Grammarinator+AFL++, afl-ts,
DIE, EASTer) usa em cima de uma árvore, não de um buffer:

| Operador | Efeito | Risco semântico |
|----------|--------|-----------------|
| **delete** | apaga subárvore | referência pendente |
| **duplicate / insert** | copia um irmão | nomes duplicados, loops maiores |
| **swap siblings** | troca dois nós do mesmo tipo | quase sempre válido |
| **replace / bank splice** | substitui por subárvore *do mesmo não-terminal* vinda do corpus | o mais útil |
| **crossover** | doador × recipiente no mesmo não-terminal | precisa de banco de subárvores |
| **shrink / hoist** | substitui nó por descendente do mesmo tipo | reduz tamanho (óraculo + minify) |
| **literal / operator tweak** | `3`→`7`, `+`→`*` | tipos costumam aguentar |
| **quantifier edit** | muda bounds de `for a..b` | terminar vs não terminar |
| **aspect-preserving** (DIE) | muta sem perder tipo / forma que já passou o typechecker | o alvo de um compilador |

Havoc (bit-flip, insert `\0`) continua útil **só** para lexer/parser.

### Geração vs mutação no Aether

O gerador actual já é *generation-based* e *type-aware* (só `i32`/`bool`,
sem recursão, divisores `1..=7`). Isso é o homólogo pobre do Csmith.

O que faltava — e o que `src/fuzz/mutate.rs` passa a fazer — é a
metade *transformation-based*:

* tweak de literais e operadores;
* delete / duplicate de statements;
* wrap `if true { stmt }`;
* ajuste de bounds de `for`;
* rename de `hN(...)`;
* inserção de `print_i32`;
* **crossover** de helpers entre dois programas;
* **shrink** por corte de helpers e linhas quando uma propriedade falha.

Não parseamos de novo a AST do Aether para mutar: os operadores
andam por linhas e blocos `{…}`. É o compromisso *afl-ts light*
sem tree-sitter. Quando o gerador crescer (structs, arrays), o passo
natural é mutar a *sketch* tipada que o gerador já constrói na cabeça,
em vez do texto.

## O que **não** vamos puxar para o `Cargo.toml`

1. **proptest / bolero / arbitrary** — edition 2024 ou MSRV alto no
   CI actual; o runner próprio já grava seed + caso.
2. **cargo-fuzz** — precisa nightly + sanitizers; o oráculo
   diferencial corre na VM, não precisa de ASan.
3. **mutatis** — MSRV 1.91.
4. **LibAFL** — potência a mais para um compilador de um ficheiro.

O sítio certo para greybox é *depois* de o frontend estar estável: um
target `fuzz/ir_roundtrip.rs` com `Arbitrary` sobre a IR, não sobre o
fonte. A IR é um tipo Rust; `#[derive(Arbitrary)]` encaixa. Rustlantis
fez exactamente isso no rustc (gerar MIR, não source).

## Como escolher um oráculo

Para compiladores, a hierarquia que vale a pena:

1. **Não pânico** — barato, já no Aether (`catch_unwind`).
2. **Well-formedness** — tokens monotônicos, IR com terminadores.
3. **Diferencial intra-compilador** — `-O0` vs `-O2` (já achou um bug
   no pass algébrico).
4. **Diferencial inter-backend** — VM vs LLVM interpretado, quando o
   emissor LLVM deixar de ser um esboço.
5. **Metamórfico** — `e` e `e + 0` / `e * 1` devem coincidir *depois*
   de desligar o próprio pass que implementa essa identidade.

(4) e (5) ainda não estão no runner.

## Comandos

```bash
aether fuzz --kind struct --iters 200 --seed 1
aether fuzz --kind diff   --iters 200
aether fuzz --kind mut    --iters 200   # havoc (bytes)
```

`--kind struct` é mutação estrutural + crossover; se o mutante ainda
tipa, corre o oráculo diferencial e tenta *shrink* na falha.

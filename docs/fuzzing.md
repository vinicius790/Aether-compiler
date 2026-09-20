# Fuzzing de propriedade

Aether não depende de `libFuzzer` nem de `proptest`. O módulo `src/fuzz`
implementa:

* um PRNG determinístico (SplitMix64);
* um gerador dirigido pela gramática, que só emite programas bem tipados e
  que terminam;
* um corpus de entradas malformadas;
* mutação havoc (bit-flip, inserção, remoção) para o lexer;
* mutação **estrutural** (literais, operadores, statements, crossover de helpers, shrink);
* um conjunto de propriedades verificadas em cada caso.

O mapa do ecossistema Rust (`proptest`, `bolero`, `cargo-fuzz`,
`mutatis`, Superion, Rustlantis) está em
[`docs/fuzzers-rust.md`](fuzzers-rust.md).

## Propriedades

| id | O que garante |
|----|----------------|
| `lexer_no_panic` | o lexer não aborta em bytes arbitrários |
| `lexer_eof` / `lexer_span` | o fluxo de tokens termina em `Eof` e os spans são monotônicos e cabem no arquivo |
| `parser_no_panic` | o parser não aborta |
| `pipeline_no_panic` | `compile_source` não aborta |
| `gen_well_typed` | programas sintéticos passam na semântica |
| `gen_tokens` | programas sintéticos não produzem `Invalid` |
| `opt_equiv_value` / `opt_equiv_stdout` | `-O0` e `-O2` devolvem o mesmo `main` e o mesmo stdout |
| `mut_no_panic` | havoc de programas válidos não derruba o compilador |
| `struct_no_panic` | mutação estrutural / crossover não aborta o pipeline |
| `struct_opt_equiv` | mutantes estruturais que ainda tipam: `-O0` == `-O2` |
| `aspect_well_typed` / `aspect_opt_equiv` | sketch tipada continua bem tipada; identidades: O0 == O2 |
| `mir_well_formed` / `mir_opt_equiv_*` | IR gerada é válida e O0 == O2 na VM |
| `greybox` | corpus + cobertura de arestas da VM; O0 == O2 em cada mutante |

## Como correr

Nos testes (`cargo test`) cada família roda dezenas de casos, com seeds
fixos, para o CI permanecer previsível.

Para uma campanha maior:

```bash
aether fuzz --iters 1000 --seed 1 --kind all
aether fuzz --iters 400 --kind diff
aether fuzz --iters 400 --kind lexer
aether fuzz --iters 200 --kind struct
aether fuzz --iters 200 --kind mut
aether fuzz --iters 200 --kind aspect
aether fuzz --iters 200 --kind mir
aether fuzz --iters 80 --kind greybox
```

Uma falha imprime a *seed* do caso. Repetir com `--seed` da campanha
reproduz a sequência; a seed do caso está no relatório para isolamento.

```bash
# o relatório diz: FAIL [opt_equiv_value] seed=12345 case=7
# a campanha usou --seed S; o caso 7 é FuzzRng::new(S ^ salt).next() ...
```

O gerador é determinístico: a mesma seed produz o mesmo programa.

## O que o gerador *não* cobre

De propósito, para manter os casos termináveis e bem tipados:

* laços `while true`;
* divisão por zero (o divisor é um literal `1..=7`);
* recursão;
* atribuição a parâmetros imutáveis;
* `if` como expressão (a linguagem não tem);
* strings/structs/arrays aleatórios (podem entrar no corpus e nas mutações).

Ampliar o gerador é bem-vindo, desde que as propriedades diferenciais
continuem válidas.

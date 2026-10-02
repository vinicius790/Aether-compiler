# Fuzzing de propriedade

Aether não depende de `libFuzzer` nem de `proptest`. O módulo `src/fuzz`
implementa:

* um PRNG determinístico (SplitMix64; a seed passa por um passo de SplitMix, por isso
  todas as seeds — incluindo 0 e pares 2k / 2k+1 — dão campanhas diferentes);
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
| `agg_*` (kind `agg`) | programas bem tipados com structs, arrays (também aninhados), `i64`/`f64`/`char`/`string`, conversões, guardas `&&` / `\|\|`, atribuição aninhada e funções com expressão final: compilam e `-O0` == `-O2` (valor e stdout) |
| `lang_*` (kind `lang`) | programas bem tipados que terminam e não dão erro de runtime com enums (0..3 cargas), `match`, `if let ... else`, tuplas, atribuição composta, operadores bit a bit / deslocamentos (incluindo >= largura), literais hex/bin/`_`/`\u{..}`, built-ins novos, `yield;`, agregados por valor com mutação na função chamada, recursão limitada e programas multi-ficheiro (`use`, ciclos, `pub` em tudo o que é importado); passam todo o conjunto de oráculos abaixo |
| `mut_no_panic` | havoc de programas válidos não derruba o compilador |
| `struct_no_panic` | mutação estrutural / crossover não aborta o pipeline |
| `struct_opt_equiv` | mutantes estruturais que ainda tipam: `-O0` == `-O2` |
| `aspect_well_typed` / `aspect_opt_equiv` | sketch tipada continua bem tipada; identidades: O0 == O2 |
| `mir_well_formed` / `mir_opt_equiv_*` | IR gerada é válida e O0 == O2 na VM |
| `greybox` | corpus + cobertura de arestas da VM; O0 == O2 em cada mutante |

### Oráculos comuns (`agg`, `lang`, `diff`)

Para cada programa sintetizado (`check_program` em `src/fuzz/mod.rs`); o prefixo da
propriedade é o do kind (`lang_opt_equiv`, ...):

| id | O que garante |
|----|----------------|
| `*_compile` / `*_well_typed` | não faz pânico, compila em -O0/-O1/-O2 sem erros |
| `*_ir_verify` | `ir::verify::verify_module` passa na IR de cada nível |
| `*_runtime_error` | o gerador prometeu ausência de erros de runtime |
| `*_opt_equiv` | valor de `main` e stdout iguais em -O0, -O1 e -O2 |
| `*_value_semantics` | o próprio programa imprime `VIOLATION` se uma cópia de agregado partilhar estado com o original (snapshots de folhas escalares) |
| `*_determinism` | duas execuções do mesmo programa compilado são idênticas (valor, stdout, passos) |
| `*_budget_equiv` | `Vm::run_budget` com orçamentos 1, 7 e 1000 (retomando em `Yielded`) dá o mesmo valor, stdout e total de passos que `run()`, em -O0 e -O2 |
| `*_fmt_idempotent` / `*_fmt_fixpoint` | (só ficheiro único) o resultado de `aether fmt` recompila para o mesmo stdout e `fmt(fmt(p)) == fmt(p)` |
| `timeout` | qualquer caso (compilação incluída) que demore mais de 5 s num thread vigiado é falha (`AETHER_FUZZ_TIMEOUT_MS`) |

Falhas de ficheiro único são minimizadas (remoção de linhas em blocos, depois de o
caso terminar; o relatório diz "minimized from N to M bytes"). Programas
multi-ficheiro são escritos num directório temporário e compilados com
`compile_files`; o relatório mostra todos os ficheiros.

`AETHER_FUZZ_NO_FMT=1` desliga o oráculo de `fmt` (ver "Achados conhecidos").
`ALLOW_MATCH_EXPR` (`src/fuzz/lang.rs`, `false` por omissão) liga a geração de `match`
como expressão quando existir.

As propriedades de lixo (lexer/parser/pipeline), de formato e de mutação
compilam também em `-O2`, não só em `-O0`. A entrada de lixo inclui sopa de
tokens com a sintaxe nova (`enum match :: => use pub yield <<= 0x \u{ ...`) e
mutação ao nível de tokens (trocar / apagar / duplicar / substituir / mover) de
programas `agg` e `lang` válidos.

## Como correr

Nos testes (`cargo test`) cada família roda dezenas de casos, com seeds
fixos, para o CI permanecer previsível.

Para uma campanha maior:

```bash
aether fuzz --iters 1000 --seed 1 --kind all
aether fuzz --iters 400 --kind diff
aether fuzz --iters 200 --kind agg
aether fuzz --iters 300 --kind lang
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

O gerador é determinístico: a mesma seed produz o mesmo programa
(`fuzz::replay_case(kind, case_seed)` reexecuta um caso a partir da seed do caso).

`cargo test --test fuzz_smoke` corre 30 casos por kind com seeds fixas;
`cargo test --lib dump_lang_sample -- --ignored --nocapture` (com `AETHER_LANG_SEED=N`)
imprime um programa `lang`.

## O que o gerador *não* cobre

De propósito, para manter os casos termináveis e bem tipados:

* laços `while true`;
* divisão por zero (o divisor é um literal `1..=7`);
* recursão;
* atribuição a parâmetros imutáveis;
* `if` como expressão (a linguagem não tem);
* o gerador clássico (`gen`, `diff`) só produz `i32`/`bool`; structs, arrays,
  `i64`/`f64`/`char`/`string`, conversões e atribuição aninhada são gerados
  por `--kind agg` (alias `aggregate`, incluído em `all`).

Ampliar o gerador é bem-vindo, desde que as propriedades diferenciais
continuem válidas.

## Achados conhecidos (campanhas 0.3)

* **Pretty-printer** (`aether fmt`): `let x: i64 = 1;` perde a anotação de tipo
  (muda o tipo de literais), um literal `f64` inteiro (`3.0`) sai como `3` (muda o
  tipo), e cada passagem acrescenta parênteses (`((-13))`, `(((p) as i64))`), logo
  `fmt(fmt(p)) != fmt(p)`; `pub enum` perde o `pub`. O teste
  `smoke_fmt_roundtrip` (tests/fuzz_smoke.rs) está `#[ignore]` até isto estar corrigido.
* `-(-2147483648)` é rejeitado com E0263 (o menos unário funde-se com o literal), mas
  `let a = -2147483648; -a` é válido: inconsistente com o resto do wrap de `i32`.
* Literais de array não propagam o tipo esperado do elemento:
  `let a: [i64; 2] = [1, 2];` dá E0230 (o gerador contorna com `(x as i64)`).
* Igualdade `==` não existe para arrays (E0300), nem para enum/struct/tupla que os
  contenham.

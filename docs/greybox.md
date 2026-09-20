# Greybox fuzzing no Aether

Greybox = mutação + *feedback de cobertura*. AFL++, libFuzzer e Honggfuzz
instrumentam o binário (SanitizerCoverage / QEMU) e preferem entradas que
pintam arestas novas. O Aether não puxa essas engines (nightly, MSRV,
sanitizers). O algoritmo cabe na VM.

## O que o `--kind greybox` faz

```
corpus ← { gen_ir() × 4 }
edges  ← cobertura O0 de cada semente

para i in 1..N:
    s ← semente ~ energia
    m ← mutate_ir(s)  ou  gen_ir()
    correr O0 com execute_with_coverage
    se m pinta aresta nova:
        corpus ← corpus ∪ {m}
        energia(s) += 1
    também: O0 == O2  (oráculo diferencial)
```

Cobertura: cada instrução da VM contribui um sítio
`(func << 32 | pc)`. A aresta é `prev.rotl(17) ⊕ site`, o mesmo
esquema que o AFL usa em miniatura. Sem bitmap de 64 KiB: um
`HashSet<u64>`.

Energia: sementes que descobriram arestas são escolhidas com mais
frequência (schedule proporcional, parente pobre do AFLFast).

## Relação com os decoy blocks

Um decoy que o `-O0` *não* executa não pinta as arestas do bloco
morto. O `-O2` pode *apagá-lo*. O greybox, ao correr O0, só vê o
lado vivo — e isso é o que queremos: o corpus cresce quando um
mutante entra num laço, chama um helper, ou toma o join depois de
um predicado opaco.

Os predicados opacos (`acc-acc==0`, `acc*0==0`) são o ponto em que
greybox e decoys se encontram. Se o const-fold *não* os reduz, O0 e
O2 ainda têm de concordar; se o reduz, o CFG encolhe e o oráculo
continua válido. Uma aresta nova no O0 depois de um `mutate_ir` que
troca `+` por `*` é sinal de que o mutante fez a VM tomar outro
caminho (por exemplo um `print` extra).

## O que não é

| Engine | Instrumentação | Neste repo |
|--------|----------------|------------|
| libFuzzer / cargo-fuzz | LLVM SanitizerCoverage | não (nightly) |
| AFL++ | PCGUARD + CmpLog | não |
| Honggfuzz | software / PMU | não |
| Aether `--kind greybox` | arestas da VM | sim |

Não há *persistent mode*, não há *cmp log*, não há minimização do
corpus para além do `mutate_ir` já existente. Quando o toolchain
subir de 1.75, o sítio certo para um target `cargo-fuzz` é
`IrModule` / `Inst`, não o fonte.

## Comando

```bash
aether fuzz --kind greybox --iters 80 --seed 1
# greybox: iters=80 corpus=… edges=… finds=… fail=0
```

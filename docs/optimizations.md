# Otimizações

Todas as transformações operam na IR própria. O LLVM, quando usado, aplica
o próprio pipeline (`opt -O2`) sobre o texto gerado — isso é separado.

## Passes

| Pass         | Efeito |
|--------------|--------|
| const-fold   | avalia `Bin`/`Un` com operandos constantes (também comparações `i64`/`f64`/`char`/`string`/`bool` e `i64` `%`; divisão com wrapping); converte `br` constante em `jmp` |
| const-prop   | segunda varredura de fold |
| algebraic    | `x+0`, `x-0`, `x*1`, `x/1`, `x*0`, `x*2 → x+x`, predicados opacos (`x-x→0`, `x==x→true`, `x<x→false`); só `i32` e `i64`, nunca `f64` (NaN) |
| local-cse    | value numbering intra-bloco (`t2 = p+1` após `t1 = p+1` → `t2 = t1`); uma entrada morre quando um registrador que menciona é redefinido (incluindo `r = r + x`, que nunca é registado) ou quando um `IndexStore`/`FieldStore` altera um operando; indexada por registrador, custo linear |
| inline       | copia folhas de um bloco para o caller; argumentos vão para registradores novos e todos os tipos de instrução são remapeados. Nunca inlina `main`, externs, funções com laços ou chamadas, nem passa de 65535 registradores no caller. Uma folha com `yield` é inlined com o `yield` |
| copy-prop    | substitui usos de `Move` pelo origem; não redirecciona a base de um store de elemento/campo e descarta aliases dos dois lados de um store (semântica de valor) |
| cf-simplify  | encadeia blocos vazios; apaga inatingíveis |
| dce          | remove instruções puras cujo destino nunca é lido. `Call`, stores, `yield`, divisão/resto inteiros por divisor que não é constante não nula e `IndexLoad` contam como efeitos (podem abortar: removê-los transformaria um erro de `-O0` em sucesso em `-O2`) |
| regalloc     | compacta registradores por função: intervalos de vida ao nível do bloco (vivo se live-in/live-out/definido/usado no bloco), coloração gulosa por ordem de aparição, parâmetros mantêm `r0..rN`; registradores nunca mencionados são descartados; a VM aloca `reg_count` slots por frame |

## Ordem

`-O1`: const-fold, copy-prop, dce.

Soundness (auditada): const-fold usa a mesma semântica que a VM (wrapping,
deslocamentos mascarados, NaN/-0.0 por IEEE, divisão por zero deixada à VM) e
só constrói strings até 64 KiB; algebraic nunca toca em `f64`; a liveness
itera até ao ponto fixo (um limite de varreduras deixava o DCE apagar
definições vivas quando o layout dos blocos corria contra o fluxo);
dead-fn não remove nada num módulo sem `main`. A regalloc usa intervalos ao
nível do bloco: um bloco enorme de temporários simultâneos (literal com mais
de ~20 mil elementos) continua a exceder os 65535 registradores e dá E0300.

`-O2`: const-fold, algebraic, cf-simplify, inline, local-cse, copy-prop,
const-prop, cf-simplify, dce, const-fold, dce — e, uma única vez depois do
ponto fixo, regalloc (a linha de stats mostra `regalloc (regs A→B)`: o total
de registradores antes e depois; a contagem de instruções não muda).

O cf-simplify corre **antes** do inline: o lowering deixa um bloco morto
depois de cada `return`, e sem limpá-lo nenhuma função parecia uma folha de
um bloco, por isso o inliner nunca disparava.

## Observabilidade

`aether optimize file.ae` imprime:

```
IR instructions: 18 → 7 (-11)
  const-fold                 18 → 12  (-6)
  ...
```

`aether dump-ir file.ae --unopt` versus `-O2` mostra o texto.

## O que não fazemos

Não inventamos speedups. Fibonacci recursivo quase não muda de forma —
o ganho mensurável aparece em expressões constantes (`examples/opt_demo.ae`)
e em código morto.

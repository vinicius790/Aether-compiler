# Otimizações

Todas as transformações operam na IR própria. O LLVM, quando usado, aplica
o próprio pipeline (`opt -O2`) sobre o texto gerado — isso é separado.

## Passes

| Pass         | Efeito |
|--------------|--------|
| const-fold   | avalia `Bin`/`Un` com operandos constantes (também comparações `i64`/`f64`/`char`/`string`/`bool` e `i64` `%`; divisão com wrapping); converte `br` constante em `jmp` |
| const-prop   | segunda varredura de fold |
| algebraic    | `x+0`, `x-0`, `x*1`, `x/1`, `x*0`, `x*2 → x+x`, predicados opacos (`x-x→0`, `x==x→true`, `x<x→false`); só `i32` e `i64`, nunca `f64` (NaN) |
| local-cse    | value numbering intra-bloco (`t2 = p+1` após `t1 = p+1` → `t2 = t1`) |
| inline       | copia folhas de um bloco para o caller; argumentos vão para registradores novos e todos os tipos de instrução são remapeados |
| copy-prop    | substitui usos de `Move` pelo origem; não redirecciona a base de um store de elemento/campo e descarta aliases dos dois lados de um store (semântica de valor) |
| cf-simplify  | encadeia blocos vazios; apaga inatingíveis |
| dce          | remove instruções puras cujo destino nunca é lido |

## Ordem

`-O1`: const-fold, copy-prop, dce.

`-O2`: const-fold, algebraic, cf-simplify, inline, local-cse, copy-prop,
const-prop, cf-simplify, dce, const-fold, dce.

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

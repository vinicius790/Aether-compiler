# Backend LLVM

Código: `src/backend/llvm.rs` (emissor e runtime), `src/driver.rs`
(`run_llvm_ir`, `run_llvm_ir_with`, `LliStatus`), `src/main.rs`
(`run --backend llvm`). Verificação: `tests/audit_llvm.rs`.

## Contrato

A VM de bytecode é a semântica de referência. Para todo o programa que o
frontend aceita, o módulo LLVM emitido imprime **os mesmos bytes** no stdout
e falha (ou não) **da mesma maneira**, com a mesma mensagem
`runtime error: ...`. `tests/audit_llvm.rs` compara as duas execuções em
casos de borda escritos à mão, em todos os `examples/*.ae` e `stdlib/*.ae`
e (teste `#[ignore]`, `AETHER_LLVM_DIFF_N=N`) em programas gerados pelos
fuzzers `agg` e `lang`. O teste é saltado quando `lli` não está no `PATH`.

O texto é LLVM IR válido para LLVM 15+ (ponteiros opacos; verificado com
`llvm-as`/`lli` 18). Não há ligação a libLLVM: o compilador escreve texto e,
para executar, chama ferramentas externas.

```bash
aether dump-llvm examples/fib.ae            # texto
aether compile examples/fib.ae --emit llvm -o fib.ll
aether run examples/fib.ae --backend llvm   # opt -O2 (se existir) + lli
```

## Forma do código

- A IR do Aether não é SSA: cada registrador tem um `alloca` no bloco
  `entry`, cada uso é um `load` e cada definição um `store` (forma pronta
  para `mem2reg`; `opt -O2` devolve SSA).
- `i32`/`i64` são 1:1, `f64` é `double`, `bool` é `i1`, `char` é `i32`.
- `string` é um ponteiro para um bloco imutável `{ i64 len, bytes, NUL }`
  (os literais são constantes; os resultados de `+` e `to_string` são
  `malloc`ados e nunca libertados).
- Arrays, structs, tuplas e enums são guardados **inline** e cada cópia
  (`let b = a;`, argumento, retorno, leitura de elemento) é um
  `llvm.memcpy`: a mesma semântica de valor da VM, sem aliasing.
- Um enum é `{ i32 tag, slot1, ... }`; um slot que as variantes tipam de
  forma diferente é um buffer de bytes com o tamanho e o alinhamento máximos.
- O runtime (helpers de string, formatação de `f64` igual ao `Display` do
  Rust, erros de runtime e os built-ins de `src/runtime/mod.rs`) é um bloco
  fixo de IR (`RUNTIME_IR`) acrescentado a cada módulo; as funções são
  `internal` e chamam-se `ae.*`, por isso nunca colidem com funções do
  utilizador (uma função do utilizador com o nome de um símbolo da libc
  usado pelo runtime é renomeada `<nome>.ae`). Um erro de runtime despeja o
  stdout, escreve `runtime error: <mensagem da VM>` no stderr e chama
  `abort()`.

## `run --backend llvm`

`aether run F --backend llvm` passa o texto por `opt-18`/`opt -S -O2` (se
existir) e executa-o com `lli-18`/`lli` (o primeiro encontrado no `PATH`):

| Situação | Saída da CLI | Exit |
|----------|--------------|------|
| `main` devolve (qualquer valor) | stdout do programa; `--stats` imprime `exit = N` (o valor de `main` módulo 256, que é o estado do processo `lli`) | 0 |
| erro de runtime (`abort()`) | stdout até ali + a linha `runtime error: ...` do runtime | 2 |
| `lli` morre com SIGSEGV (pilha nativa esgotada por recursão profunda) | `runtime error: the program crashed under lli (SIGSEGV; ...)` | 2 |
| mais de `--timeout` segundos (omissão 10) | `runtime error: time limit exceeded (N s; ...)`; `lli` é morto | 2 |
| `lli` não encontrado | `error: --backend llvm needs LLVM's lli ...` | 1 |
| `opt`/`lli` rejeitam o módulo, ou `lli` morre com outro sinal | `llvm backend error: ...` | 1 |

## Limites conhecidos (diferenças face à VM)

- Não há orçamento de instruções nem de profundidade: `--max-steps` e
  `--max-depth` só se aplicam à VM; o único limite é `--timeout`. Uma
  recursão infinita que a VM pára com `call stack overflow` pode, depois
  de `opt -O2` a transformar num laço, só terminar com o `--timeout`; sem
  `opt`, rebenta a pilha nativa (SIGSEGV, reportado como erro de runtime).
- Quando `lli` é morto pelo `--timeout`, o stdout que o programa ainda tinha
  em buffer perde-se (a VM imprime o stdout parcial).
- `yield;` não faz nada (não há `run_budget`).
- Uma `extern fn` não tem implementação: chamá-la é
  `runtime error: extern function `f` has no implementation (unresolved symbol)`
  (a VM diz `... has no implementation in the VM`; a ligação a funções do
  anfitrião só existe na VM, `host::Host`).
- As strings criadas em runtime nunca são libertadas (o programa é curto).
- O texto LLVM não é uma ABI estável entre versões.

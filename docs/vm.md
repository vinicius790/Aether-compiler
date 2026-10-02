# VM Aether

Máquina de registradores. Cada frame tem `nregs` slots `Value`
(`nregs` até 65535; registradores `u16`).

## Valores

`I32`, `I64`, `F64`, `Bool`, `Str`, `Char`, `Unit`, `Array`, `Object`.

## Chamadas

`Call func dest args` empurra um frame. Os primeiros `arity` registradores
recebem os argumentos. `Ret` escreve no `ret_reg` do chamador.

## Semântica de valor

Arrays e structs copiam-se por valor (cópia profunda). A aritmética inteira,
incluindo a divisão, usa wrapping: `i32::MIN / -1 == i32::MIN`.

## Nativos

| id | nome       |
|----|------------|
| 0  | print      |
| 1  | println    |
| 2  | print_i32  |
| 3  | print_i64  |
| 4  | print_f64  |
| 5  | print_bool |
| 6  | len        |
| 7  | assert     |

## Limites

- 50 milhões de instruções por execução (configurável)
- 10 mil frames de chamada
- divisão por zero e índice inválido abortam com `VmError`
- chamar uma `extern fn` que a VM não implementa é `VmError` (antes
  comportava-se como `print`)
- `len` conta caracteres (valores escalares Unicode), não bytes

Num erro de runtime a CLI imprime o stdout produzido até ali, depois
`runtime error: ...`, e sai com código 2.

## Inspeção

`aether dump-bytecode` e `aether run --stats` (passos executados).

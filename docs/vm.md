# VM Aether

Máquina de registradores. Cada frame tem `nregs` slots `Value`.

## Valores

`I32`, `I64`, `F64`, `Bool`, `Str`, `Char`, `Unit`, `Array`, `Object`.

## Chamadas

`Call func dest args` empurra um frame. Os primeiros `arity` registradores
recebem os argumentos. `Ret` escreve no `ret_reg` do chamador.

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

## Inspeção

`aether dump-bytecode` e `aether run --stats` (passos executados).

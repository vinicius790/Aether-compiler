# ISA da VM Aether

Este documento é a referência da máquina de registradores. O assembler
está em `src/backend/bytecode.rs`. O interpretador está em `src/vm/mod.rs`.
A ISA **não é estável** entre versões (ver CHANGELOG).

## Modelo

- Um módulo tem um pool de funções, um pool de strings e um índice `entry`
  (`main`).
- Cada função tem `arity`, `nregs` e um `Vec<Op>`.
- Registradores são `u16` (até 65535 por frame). Se uma função precisar de
  mais, o assembler devolve erro de compilação; nunca trunca.
- O assembler **valida** a IR antes de emitir código e recusa (E0300) o que
  faria a VM sair do frame ou da função: registrador `>= reg_count`, salto
  para bloco inexistente, blocos com id repetido, função sem blocos, chamada
  com número de argumentos diferente da aridade do callee, função repetida,
  parâmetros que não ocupam `r0..rN`, objecto com mais de 65535 campos,
  índice de campo `> 65535` e array literal com mais de 2^28 elementos
  (`MAX_ARRAY_LEN`). IR bem formada nunca dispara estes erros.
- Chamadas empilham um `Frame { func, pc, regs, ret_reg }`.
- Não há heap partilhado entre frames para escalares. Arrays e structs
  vivem como `Value::Array` / `Value::Object` no registrador.

## Imediatos

`LoadImm` carrega um `Immediate`:

| Variante | Significado |
|----------|-------------|
| `I32` | literal i32 |
| `I64` | literal i64 |
| `F64` | bits IEEE (`to_bits`) |
| `Bool` | |
| `Str(i)` | índice no pool `module.strings` |
| `Char` | codepoint u32 |
| `Unit` | `()` |

`LoadStr` é o atalho tipado para strings.

## Aritmética i32

`AddI32`, `SubI32`, `MulI32`, `DivI32`, `RemI32`, `NegI32`.

Divisão e resto por zero são erro de runtime (`VmError::Runtime`).
A aritmética inteira, incluindo a divisão, usa wrapping
(`i32::MIN / -1 == i32::MIN`, sem pânico), na VM e no fold do compilador.

## Aritmética i64 e f64

Conjunto paralelo, incluindo `RemI64` e `NegI64`. `NegF64` existe; não há
`RemF64`.

## Comparações

Opcodes especializados: i32 `Eq Ne Lt Le Gt Ge`; i64 `Eq Lt`; f64 `Eq Lt`;
bool `Eq`, mais `AndBool` `OrBool` `NotBool`.

Todas as outras comparações que a sema aceita usam o opcode genérico
`Cmp { op }`, com `op` em `Eq Ne Lt Le Gt Ge`, sobre i64, f64, bool, char e
string (bool e string só `Eq`/`Ne`).

O lowering da IR mapeia `BinOp` + `Type` para um destes opcodes. Uma
combinação (operador, tipo) sem opcode é erro de compilação (E0300);
nunca vira `Nop`.

## Controlo

| Op | Efeito |
|----|--------|
| `Jump t` | `pc = t` |
| `JumpIf c t` | se `regs[c]` é truthy, `pc = t` (disassembly `jnz`) |
| `JumpIfNot c t` | o inverso (disassembly `jz`; o assembler só emite `JumpIf`) |
| `Call f, dest, args` | frame novo; args copiados para r0.. |
| `CallNative id, dest, args` | tabela em `call_native` |
| `Ret src` | escreve `ret_reg` no caller e desempilha |
| `RetVoid` | desempilha sem valor |
| `Nop` | |

Alvos de jump são índices no `Vec<Op>` da **mesma** função.

## Conversões

`CastI32ToI64`, `CastI64ToI32` (trunca), `CastI32ToF64`, `CastF64ToI32`,
`CastBoolToI32`, `CastI64ToF64`, `CastF64ToI64`, `CastBoolToI64`,
`CastCharToI32`, `CastI32ToChar`. Cobrem todos os pares de `can_cast_to`
(`ty.rs`).

## Agregados

`AllocArr dest, len` — vector de `len` `I32(0)`, preenchido depois com
`StoreIdx` (`len > 2^28` é erro de runtime, nunca abort por falta de
memória).  
`LoadIdx` / `StoreIdx` — fora de intervalo é erro de runtime; o índice tem de
ser `I32` e a base um array (`LoadIdx` aceita também `Str`), senão
`VmError::Runtime` (`cannot index a value of type unit`).  
`AllocObj dest, fields` — struct, tupla ou enum (`fields: u16`).  
`LoadField` / `StoreField` — índice estático `u16`; base que não é objecto,
ou índice fora do objecto, é `VmError::Runtime` (antes devolvia `Unit` e
descartava a escrita em silêncio).  
`Concat` — strings; os dois operandos têm de ser `Str` e o resultado não pode
passar de 256 MiB (`MAX_STRING_BYTES`), senão erro de runtime.  
`Cmp` com `Eq`/`Ne` compara arrays e objectos elemento a elemento (floats
IEEE: `NaN != NaN`); a ordem entre agregados é erro.

Arrays e structs têm semântica de valor: copiá-los para outro registrador
faz cópia profunda.

## Bits, `yield`

`BitAnd`/`BitOr`/`BitXor`/`Shl`/`Shr`/`NotInt` operam em `i32` ou `i64`
conforme o valor do registrador esquerdo; o deslocamento é mascarado
(`& 31` / `& 63`) e `Shr` é aritmético. `Yield` suspende `Vm::run_budget`
(devolve `Step::Yielded`) e é ignorado por `run()`. Natives 8..=20 estão
listados em `src/runtime/mod.rs`; os índices de funções do utilizador no
módulo começam depois da tabela de natives.

## Natives

Ver `src/runtime/mod.rs`. Ids estáveis só dentro da mesma versão. Chamar
uma `extern fn` que a VM não implementa é erro de runtime.

## Perfil e digest

`VmOptions.profile` incrementa `calls[func]` em cada `Call` e na entrada
de `main`. `digest` é FNV-1a de `"{value}\n{stdout}"`.

## Natives estritos

`CallNative` verifica a etiqueta de cada argumento (`print_i32` exige `I32`,
`len` exige `Str` ou `Array`, ...). Um argumento de outro tipo, ou em falta, é
`VmError::Native("`abs` expects i32 for argument 1, got unit")`. Os opcodes
aritméticos continuam a ler registradores pela conversão leniente `as_*`: o
compilador nunca emite operandos mal tipados, e os valores que vêm do host
são validados na fronteira (ver `vm.md`).

## Limites

`max_steps` 50_000_000, `max_call_depth` 10_000. O fuzzer depende disto.

# ISA da VM Aether

Este documento é a referência da máquina de registradores. O assembler
está em `src/backend/bytecode.rs`. O interpretador está em `src/vm/mod.rs`.
A ISA **não é estável** entre versões (ver CHANGELOG).

## Modelo

- Um módulo tem um pool de funções, um pool de strings e um índice `entry`
  (`main`).
- Cada função tem `arity`, `nregs` e um `Vec<Op>`.
- Registradores são `u8` (no máximo 256 por frame). A IR de compilação
  pode ter mais; o assembler falha se `reg_count > 255`.
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
Overflow de i32 na VM usa wrapping nos folds do otimizador; a VM Rust
usa operadores nativos (pode panic em debug em `i32` overflow — os
testes evitam isso com valores pequenos). O fold do compilador usa
`wrapping_*` de propósito.

## Aritmética i64 e f64

Conjunto paralelo. `NegF64` existe; não há `RemF64`.

## Comparações

i32: `Eq Ne Lt Le Gt Ge`.  
i64: `Eq Lt`.  
f64: `Eq Lt`.  
bool: `Eq`, mais `AndBool` `OrBool` `NotBool`.

O lowering da IR mapeia `BinOp` + `Type` para um destes opcodes.
Comparações em falta para i64/f64 (Ge, etc.) devem ser expandidas no
assembler como `Not` de `Lt` se algum dia forem emitidas — hoje o
front-end só gera o que o assembler cobre para os exemplos oficiais.

## Controlo

| Op | Efeito |
|----|--------|
| `Jump t` | `pc = t` |
| `JumpIf c t` | se `regs[c]` é truthy, `pc = t` |
| `JumpIfNot c t` | o inverso |
| `Call f, dest, args` | frame novo; args copiados para r0.. |
| `CallNative id, dest, args` | tabela em `call_native` |
| `Ret src` | escreve `ret_reg` no caller e desempilha |
| `RetVoid` | desempilha sem valor |
| `Nop` | |

Alvos de jump são índices no `Vec<Op>` da **mesma** função.

## Conversões

`CastI32ToI64`, `CastI64ToI32` (trunca), `CastI32ToF64`, `CastF64ToI32`,
`CastBoolToI32`.

## Agregados

`AllocArr dest, len` — vector de `len` `Unit`s, preenchido depois com
`StoreIdx`.  
`LoadIdx` / `StoreIdx` — fora de intervalo é erro de runtime.  
`AllocObj dest, fields` — struct.  
`LoadField` / `StoreField` — índice estático `u8`.  
`Concat` — strings.

## Natives

Ver `src/runtime/mod.rs`. Ids estáveis só dentro da mesma versão.

## Perfil e digest

`VmOptions.profile` incrementa `calls[func]` em cada `Call` e na entrada
de `main`. `digest` é FNV-1a de `"{value}\n{stdout}"`.

## Limites

`max_steps` 50_000_000, `max_call_depth` 10_000. O fuzzer depende disto.

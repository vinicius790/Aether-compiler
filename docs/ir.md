# IR Aether

## Função

```
fn add(%0: i32 /* a */, %1: i32 /* b */) -> i32 {
bb0:
  %2 = add.i32 %0, %1
  ret %2
}
```

## Instruções

`LoadConst`, `Move`, `Bin`, `Un`, `Call`, `Cast`, `IndexLoad`,
`IndexStore`, `FieldLoad`, `FieldStore`, `AllocArray`, `AllocStruct`, `Nop`.

## Terminadores

`Jump`, `Branch`, `Return`, `Unreachable`.

Todo bloco termina com exatamente um terminador. Blocos inatingíveis
podem ser eliminados por `cf-simplify`.

## Semântica

Avaliação sequencial dentro do bloco. `Call` avalia argumentos da
esquerda para a direita. `Bin`/`Un` seguem as regras de `ty::binop_result`.
Não há memória global. Locais vivem em registradores; arrays/structs são
valores alocados por `Alloc*`.

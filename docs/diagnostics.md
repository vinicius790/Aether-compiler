# Diagnósticos

Todos os códigos que o compilador emite, com a mensagem tal como sai (para o
programa de exemplo) e um programa mínimo que a provoca. `tests/audit_a3.rs`
compila cada exemplo desta página e verifica o código e a mensagem, por isso
a tabela não pode ficar desactualizada sem o teste falhar.

Formato na CLI (cores só num terminal, ver [`cli.md`](cli.md)):

```
error: [E0235] returning `bool` from function of type `i32`
  --> main.ae:1:20
     |
   1 | fn main() -> i32 { return true; }
     |                    ^^^^^^^^^^^^
```

As colunas contam caracteres; os tabs da linha são copiados para a linha de
`^` e os caracteres largos (CJK, emoji) contam duas células. Caracteres de
controlo do fonte (um `\r` solto, um ESC dentro de uma string) aparecem
como `�` em vez de irem para o terminal; na mensagem, nas notas, na ajuda e
no nome do ficheiro aparecem escapados (`` unexpected character `\u{1b}` ``). Erros (`error`) impedem a
geração de código e dão `exit 1`; avisos (`warning`) não. Vários erros do
mesmo ficheiro saem todos; a análise semântica só corre se o léxico e a
sintaxe passarem.

## Léxico (`E00xx`)

| Código | Mensagem | Exemplo |
|--------|----------|---------|
| `E0001` | unexpected character `@` | `fn main() -> i32 { let x = 1 @ 2; return 0; }` |
| `E0002` | unterminated string literal | `fn main() -> i32 { let s = "abc; return 0; }` |
| `E0003` | empty character literal | `fn main() -> i32 { let c = ''; return 0; }` |
| `E0004` | unterminated block comment | `fn main() -> i32 { return 0; } /* open` |
| `E0005` | invalid unicode escape | `fn main() -> i32 { let s = "\u{110000}"; return 0; }` |
| `E0006` | unknown escape sequence `\q` | `fn main() -> i32 { let s = "\q"; return 0; }` |

## Sintaxe (`E01xx`)

| Código | Mensagem | Exemplo |
|--------|----------|---------|
| `E0100` | expected expression, found `;` | `fn main() -> i32 { return 1 +; }` |
| `E0101` | nesting too deep (limit 256) | `return` com 300 parênteses aninhados |

## Semântica (`E02xx`)

| Código | Mensagem | Exemplo |
|--------|----------|---------|
| `E0201` | duplicate struct `A` | `struct A { x: i32 } struct A { y: i32 } fn main() -> i32 { return 0; }` |
| `E0202` | duplicate field `x` | `struct A { x: i32, x: i32 } fn main() -> i32 { return 0; }` |
| `E0203` | duplicate function `f` | `fn f() {} fn f() {} fn main() -> i32 { return 0; }` |
| `E0204` | duplicate variant `A` | `enum E { A, A } fn main() -> i32 { return 0; }` |
| `E0205` | recursive type `S` has infinite size | `struct S { s: S } fn main() -> i32 { return 0; }` |
| `E0206` | enum `E` has no variants | `enum E { } fn main() -> i32 { return 0; }` |
| `E0210` | program is missing an entry point `fn main()` | `fn f() {}` |
| `E0211` | `main` must not take parameters | `fn main(x: i32) -> i32 { return 0; }` |
| `E0212` | `main` must return `i32` or `unit` | `fn main() -> bool { return true; }` |
| `E0220` | function `f` may not return a value of type `i32` on all paths | `fn f(x: i32) -> i32 { if x > 0 { return 1; } } fn main() -> i32 { return f(1); }` |
| `E0221` | tail expression has type `bool`, but `f` returns `i32` | `fn f() -> i32 { true } fn main() -> i32 { return f(); }` |
| `E0230` | cannot assign `bool` to variable of type `i32` | `fn main() -> i32 { let x: i32 = true; return 0; }` |
| `E0231` | variable `x` needs a type annotation or initializer | `fn main() -> i32 { let x; return 0; }` |
| `E0232` | enum variable `e` needs an initializer | `enum E { A } fn main() -> i32 { let e: E; return 0; }` |
| `E0233` | cannot assign `bool` to `i32` | `fn main() -> i32 { let mut x = 1; x = true; return 0; }` |
| `E0234` | missing return value (expected `i32`) | `fn main() -> i32 { return; }` |
| `E0235` | returning `bool` from function of type `i32` | `fn main() -> i32 { return true; }` |
| `E0236` | condition has type `i32`, expected `bool` | `fn main() -> i32 { if 1 { } return 0; }` |
| `E0237` | while-condition has type `i32`, expected `bool` | `fn main() -> i32 { while 1 { } return 0; }` |
| `E0238` | for-range end must be `i32`, found `f64` | `fn main() -> i32 { for i in 0..2.0 { } return 0; }` |
| `E0239` | `break` outside of a loop | `fn main() -> i32 { break; return 0; }` |
| `E0240` | `continue` outside of a loop | `fn main() -> i32 { continue; return 0; }` |
| `E0241` | cannot assign to immutable binding `x` | `fn main() -> i32 { let x = 1; x = 2; return 0; }` |
| `E0242` | invalid assignment target | `fn main() -> i32 { 1 = 2; return 0; }` |
| `E0243` | cannot find value `y` in this scope | `fn main() -> i32 { return y; }` |
| `E0244` | operator `+` is not defined for `i32` and `bool` | `fn main() -> i32 { return 1 + true; }` |
| `E0245` | unary `-` is not defined for `bool` | `fn main() -> i32 { let b = -true; return 0; }` |
| `E0246` | array index must be an integer | `fn main() -> i32 { let a = [1]; return a[true]; }` |
| `E0247` | cannot index into `i32` | `fn main() -> i32 { let a = 1; return a[0]; }` |
| `E0248` | no field `y` on type `S` | `struct S { x: i32 } fn main() -> i32 { let s = S { x: 1 }; return s.y; }` |
| `E0249` | array element has type `bool`, expected `i32` | `fn main() -> i32 { let a = [1, true]; return 0; }` |
| `E0250` | cannot infer type of empty array | `fn main() -> i32 { let a = []; return 0; }` |
| `E0251` | field `x` has type `i32`, found `bool` | `struct S { x: i32 } fn main() -> i32 { let s = S { x: true }; return 0; }` |
| `E0252` | struct `S` has no field `y` | `struct S { x: i32 } fn main() -> i32 { let s = S { x: 1, y: 2 }; return 0; }` |
| `E0253` | missing field `y` in `S` literal | `struct S { x: i32, y: i32 } fn main() -> i32 { let s = S { x: 1 }; return 0; }` |
| `E0254` | unknown struct `T` | `fn main() -> i32 { let s = T { x: 1 }; return 0; }` |
| `E0255` | cannot cast `string` to `i32` | `fn main() -> i32 { let s = "a" as i32; return 0; }` |
| `E0256` | calling computed function values is not supported | `fn main() -> i32 { return (1)(2); }` |
| `E0257` | function `f` takes 1 argument(s), found 2 | `fn f(x: i32) -> i32 { return x; } fn main() -> i32 { return f(1, 2); }` |
| `E0258` | argument 1 to `f` has type `bool`, expected `i32` | `fn f(x: i32) -> i32 { return x; } fn main() -> i32 { return f(true); }` |
| `E0260` | unknown function `nope` | `fn main() -> i32 { return nope(); }` |
| `E0261` | unknown type `Foo` | `fn main() -> i32 { let x: Foo = 1; return 0; }` |
| `E0262` | array length does not fit in `i32` | `fn main() -> i32 { let a: [i32; 4294967296]; return 0; }` |
| `E0263` | integer literal `2147483648` is out of range for `i32` | `fn main() -> i32 { let x = 2147483648; return 0; }` |
| `E0264` | function `g` cannot be used as a value | `fn g() -> i32 { return 1; } fn main() -> i32 { let h = g; return 0; }` |
| `E0265` | unknown enum `Nope` | `fn main() -> i32 { let x = Nope::A; return 0; }` |
| `E0266` | enum `E` has no variant `B` | `enum E { A } fn main() -> i32 { let x = E::B; return 0; }` |
| `E0267` | variant `E::A` takes 1 value(s), but 0 were given | `enum E { A(i32) } fn main() -> i32 { let x = E::A; return 0; }` |
| `E0268` | float literals cannot be used as patterns | `fn main() -> i32 { let x = 1.0; match x { 1.0 => {} _ => {} } return 0; }` |
| `E0269` | cannot destructure `(i32, i32, i32)` into 2 names | `fn main() -> i32 { let (a, b) = (1, 2, 3); return 0; }` |
| `E0270` | non-exhaustive match on `E`: pattern `E::B` not covered | `enum E { A, B } fn main() -> i32 { match E::A { E::A => {} } return 0; }` |
| `E0271` | duplicate match arm: an earlier arm has the same pattern | `enum E { A, B } fn main() -> i32 { match E::A { E::A => {} E::A => {} _ => {} } return 0; }` |
| `E0273` | match arms have incompatible types: expected `bool`, found `i32` | `fn main() -> i32 { let x = match 1 { 1 => 2, _ => true }; return 0; }` |
| `E0274` | `a` is bound more than once in the same pattern | `fn main() -> i32 { match (1, 2) { (a, a) => {} } return 0; }` |

`E0259` (`` `x` is not a function ``) existe no código mas não é alcançável:
chamar um nome que não é uma função (uma variável, por exemplo) dá `E0260`.

## Avisos (`Wxxxx`)

| Código | Mensagem | Exemplo |
|--------|----------|---------|
| `W0232` | shadows existing binding `x` | `fn main() -> i32 { let x = 1; let x = 2; return x; }` |
| `W0272` | unreachable match arm: earlier arms already cover every value it matches | `fn main() -> i32 { match true { _ => {} true => {} } return 0; }` |

## Módulos (`E028x`)

| Código | Mensagem | Quando |
|--------|----------|--------|
| `E0280` | unresolved import: cannot read missing.ae: No such file or directory (os error 2) (imported from main.ae:1) | `use "missing";` sem `missing.ae` ao lado do ficheiro que importa; também um diretório, um ficheiro acima de 8 MiB, não UTF-8, ou `use "";` (`empty import path`) |
| `E0281` | `` `priv` is private to `p.ae` `` | `main.ae` faz `use "p";` e chama `priv()`, definida em `p.ae` sem `pub`; vem com a nota `` function `priv` is defined at p.ae:1:4 without `pub` `` e a ajuda `` mark it `pub` in p.ae `` |

## Backend (`E0300`)

`E0300` vem do assembler de bytecode: uma combinação que a VM não sabe
executar ou um limite da VM ultrapassado (por exemplo
`` function `main` needs 70003 registers; the VM supports at most 65535 ``
quando mais de 65535 valores estão vivos ao mesmo tempo mesmo depois da
compactação de registradores (que todos os níveis `-O` fazem quando é
preciso); um
literal de array com mais de 2^28 elementos ou um objecto com mais de 65535
campos). Programas que a análise semântica aceita só o encontram nestes
limites.

## Erros que não são diagnósticos

| Saída | Exit | Causa |
|-------|------|-------|
| `cannot read F: ...` | 1 | ficheiro em falta, diretório, não UTF-8 (`file is not valid UTF-8 (first bad byte at offset N)`) |
| `refusing to compile F: file is larger than 8 MiB` | 1 | limite por ficheiro |
| `missing command`, `missing file operand`, `unknown option ...`, `invalid optimization level ...`, `... needs a value` | 1 | uso errado da CLI ([`cli.md`](cli.md)) |
| `runtime error: division by zero` | 2 | divisão ou resto inteiro por zero |
| `runtime error: array index N out of bounds` / `string index out of bounds` | 2 | índice fora do intervalo |
| `runtime error: assertion failed` | 2 | `assert(false)` |
| `runtime error: call stack overflow` | 2 | mais de `--max-depth` frames (omissão 10 000) |
| `runtime error: execution exceeded the instruction step limit` | 2 | mais de `--max-steps` instruções (omissão 50 000 000) |
| `runtime error: time limit exceeded` | 2 | `--timeout` |
| ``runtime error: extern function `f` has no implementation in the VM`` | 2 | `extern fn` sem *binding* do anfitrião |
| `runtime error: string of N bytes exceeds the limit of 268435456` | 2 | string acima de 256 MiB |

Num erro de runtime o stdout produzido até ali é impresso antes da linha
`runtime error: ...`.

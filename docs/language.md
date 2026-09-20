# Especificação da linguagem Aether 0.1

## Propósito

Aether existe para tornar visível cada etapa de um compilador otimizante.
O desenho privileia:

1. uma gramática pequena o bastante para ter especificação fechada;
2. um sistema de tipos estático sem inferência global;
3. semântica determinística, sem comportamento indefinido à la C;
4. mapeamento direto para uma IR de três endereços.

## Paradigma

Imperativo, procedural, com tipos nominais para `struct` e estruturais para
arrays. Avaliação eager. Funções não são valores.

## Filosofia

- Explícito vence implícito, exceto na anotação de `let` quando o
  inicializador determina o tipo.
- Sem sobrecarga de operadores. `+` em `i32` e `+` em `string` são regras
  distintas documentadas, não traits.
- Sem pré-processador.
- Erros de tipo são erros de compilação; a VM só vê programas bem tipados.

## Unidades de compilação

Um arquivo `.ae` contém zero ou mais itens (`fn`, `struct`, `extern fn`).
O ponto de entrada é `fn main() -> i32` ou `fn main() -> unit`.

## Tipos

| Tipo     | Descrição                         | Tamanho VM |
|----------|-----------------------------------|------------|
| `unit`   | ausência de valor                 | 0          |
| `bool`   | `true` / `false`                  | 1          |
| `i32`    | inteiro sinalizado de 32 bits     | 4          |
| `i64`    | inteiro sinalizado de 64 bits     | 8          |
| `f64`    | IEEE-754 binário 64               | 8          |
| `char`   | code point Unicode (armazenado u32) | 4        |
| `string` | sequência UTF-8 imutável          | handle     |
| `[T; N]` | array de `N` elementos `T`        | N × \|T\|  |
| `S`      | struct nominal                    | soma       |

Não há ponteiros, referências nem genéricos nesta versão.

### Compatibilidade

Atribuição exige igualdade de tipo. Não há promoção implícita `i32 → i64`.
Conversões explícitas usam `e as T` e só as pares listadas em `ty.rs`
(`can_cast_to`) são legais.

### Inferência

`let x = 1;` produz `i32`. `let x: i64 = 1;` produz `i64` porque o literal
inteiro é flexível entre `i32` e `i64` quando o contexto espera um inteiro.

## Operadores e precedência

Do mais frouxo ao mais apertado:

| Nível | Operadores           | Associatividade |
|-------|----------------------|-----------------|
| 1     | `\|\|`               | esquerda        |
| 2     | `&&`                 | esquerda        |
| 3     | `== !=`              | esquerda        |
| 4     | `< <= > >=`          | esquerda        |
| 5     | `+ -`                | esquerda        |
| 6     | `* / %`              | esquerda        |
| 9     | `as`                 | esquerda        |
| 12    | prefixos `- !`       | direita         |
| 13    | chamada, `[]`, `.`   | esquerda        |

Aritmética exige operandos do mesmo tipo numérico. `%` não se aplica a `f64`.
`+` também concatena `string`. Comparações produzem `bool`.

Overflow de inteiros na VM é wrapping (two's complement). Divisão por zero
é erro de runtime.

## Declarações

```
let [mut] nome [: tipo] [= expr];
```

Reatribuição só é permitida em bindings `mut` e em elementos de array /
campos de struct obtidos por indexação.

## Funções

```
fn nome(p1: T1, p2: T2) -> R { ... }
extern fn nome(p1: T1) -> R;
```

Argumentos são passados por valor. Structs e arrays são valores (cópia rasa
da raiz na VM). Toda função com tipo de retorno ≠ `unit` deve retornar em
todos os caminhos.

## Controle de fluxo

- `if expr { ... } [else { ... }]` — `expr: bool`
- `while expr { ... }`
- `for nome in expr .. expr { ... }` — intervalo semiaberto `[start, end)`
  em `i32`; a variável de iteração é `mut i32`
- `break` / `continue` apenas dentro de laço
- `return [expr];`

## Structs

```
struct Point { x: i32, y: i32 }
let p = Point { x: 1, y: 2 };
p.x
```

Campos são públicos. Literal deve nomear todos os campos.

## Arrays

```
let a: [i32; 4] = [1, 2, 3, 4];
a[i]
```

Índice fora do intervalo é erro de runtime.

## Strings

Literais `"..."` com escapes `\n \t \r \0 \\ \"`. Concatenação `+`.
`len(s)` devolve `i32`. Indexação devolve `char`.

## Built-ins

`print`, `println`, `print_i32`, `print_i64`, `print_f64`, `print_bool`,
`len`, `assert`. São funções do runtime, não palavras-chave.

## Modelo de memória

A VM armazena valores em registradores por frame. Arrays e structs são
`Value::Array` / `Value::Object` no heap do processo hospedeiro (Rust).
Não há aliasing observável além da mutação do valor no registrador local.
Não há lifetime nem GC explícito: o `Drop` do frame libera as árvores.

## Modelo de execução

1. Compilação AOT para bytecode de registradores.
2. A VM interpreta o bytecode com limite de passos e de profundidade.
3. `main` devolve o código de saída lógico (impresso só com `--stats`).

## Erros

Erros de compilação abortam a geração de código. Erros de runtime
(`division by zero`, bounds, `assert`, overflow de pilha) encerram a VM
com mensagem; não há exceções na linguagem.

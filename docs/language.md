# Especificação da linguagem Aether 0.2

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
(`can_cast_to`) são legais: `i32 ↔ i64`, `i32`/`i64 → f64`, `f64 → i32`/`i64`,
`bool → i32`/`i64`, `char → i32` e `i32 → char`. Todas executam na VM.

### Inferência

`let x = 1;` produz `i32`. `let x: i64 = 1;` produz `i64` porque o literal
inteiro é flexível entre `i32` e `i64` quando o contexto espera um inteiro.

O tipo esperado propaga-se através do menos unário, de parênteses e dos
operadores aritméticos e de comparação; um literal inteiro nu adopta o tipo
do outro operando (`1 + a` com `a: i64` é `i64`).

Um literal negativo é um único literal: `let y: i64 = -1;` é válido e
`-2147483648` é um `i32` válido. Um literal inteiro cujo tipo resulte `i32`
e que não caiba em 32 bits é erro (E0263); em contexto `i64` o literal pode
usar os 64 bits.

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
`+` também concatena `string`. Comparações produzem `bool`: `i32`, `i64`,
`f64` e `char` aceitam as seis (`== != < <= > >=`); `bool` e `string`
aceitam `==` e `!=`.

`&&` e `||` fazem curto-circuito: o operando direito só é avaliado quando
é preciso.

Overflow de inteiros na VM é wrapping (two's complement), incluindo a
divisão (`i32::MIN / -1 == i32::MIN`, sem pânico). Divisão por zero é erro
de runtime.

Funções não são valores: usar o nome de uma função fora de uma chamada é
erro (E0264).

Expressões e blocos aninhados mais fundo que 256 níveis são diagnosticados
pelo parser (E0101).

## Declarações

```
let [mut] nome [: tipo] [= expr];
```

Reatribuição só é permitida em bindings `mut` e em elementos de array /
campos de struct obtidos por indexação, também aninhados
(`a[i][j] = v`, `o.inner.x = v`).

## Funções

```
fn nome(p1: T1, p2: T2) -> R { ... }
extern fn nome(p1: T1) -> R;
```

Argumentos são passados por valor. Structs e arrays têm semântica de valor:
`let b = a;` copia, e na VM a cópia é profunda (arrays aninhados e structs
dentro de structs também). Toda função com tipo de retorno ≠ `unit` deve
retornar em todos os caminhos.

A expressão final de um corpo de função, sem `;`, é o valor devolvido e
tem de ter o tipo de retorno (E0221). Em qualquer outro bloco a expressão
final é avaliada como instrução.

Chamar uma `extern fn` que a VM não implementa é erro de runtime.

## Controle de fluxo

- `if expr { ... } [else { ... }]` — `expr: bool`
- `while expr { ... }`
- `for nome in expr .. expr { ... }` — intervalo semiaberto `[start, end)`
  em `i32` (os limites têm de ser `i32`, E0238); a variável de iteração é
  `mut i32` e só existe no corpo do laço
- `break` / `continue` apenas dentro de laço
- `return [expr];`

## Structs

```
struct Point { x: i32, y: i32 }
let p = Point { x: 1, y: 2 };
p.x
```

Campos são públicos. Literal deve nomear todos os campos, em qualquer
ordem: os inicializadores são avaliados na ordem do fonte e guardados na
posição declarada.

## Arrays

```
let a: [i32; 4] = [1, 2, 3, 4];
a[i]
```

Índice fora do intervalo é erro de runtime.

## Strings

Literais `"..."` com escapes `\n \t \r \0 \\ \"`. Concatenação `+`.
`len(s)` devolve `i32` e conta valores escalares Unicode (chars), como a
indexação, não bytes. Indexação devolve `char`.

## Built-ins

`print`, `println`, `print_i32`, `print_i64`, `print_f64`, `print_bool`,
`len`, `assert`. São funções do runtime, não palavras-chave.

## Modelo de memória

A VM armazena valores em registradores por frame. Arrays e structs são
`Value::Array` / `Value::Object` no heap do processo hospedeiro (Rust).
Como arrays e structs têm semântica de valor, não há aliasing observável
entre variáveis: `let b = a;` copia a árvore `Value`. (No emissor LLVM,
que é só de estudo, copiar um agregado copia o ponteiro e há aliasing.)
Não há lifetime nem GC explícito: o `Drop` do frame libera as árvores.

## Modelo de execução

1. Compilação AOT para bytecode de registradores.
2. A VM interpreta o bytecode com limite de passos e de profundidade.
3. `main` devolve o código de saída lógico (impresso só com `--stats`).

## Erros

Erros de compilação abortam a geração de código. Erros de runtime
(`division by zero`, bounds, `assert`, overflow de pilha, chamada de
`extern fn` não implementada) encerram a VM com mensagem; não há exceções
na linguagem. A CLI imprime o stdout produzido até ali antes de
`runtime error: ...` e sai com código 2.

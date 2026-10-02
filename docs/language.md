# Especificação da linguagem Aether 0.3

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

Um arquivo `.ae` contém zero ou mais itens (`fn`, `struct`, `extern fn`,
`use`). O ponto de entrada é `fn main() -> i32` ou `fn main() -> unit`. Um
programa pode estender-se por vários ficheiros através de `use` (ver
[Módulos](#módulos)).

## Módulos

```
use "relative/path.ae";
```

`use` é um item de topo que importa **todos** os itens de outro ficheiro
para o programa. Semântica (0.3):

- **Caminhos relativos ao ficheiro que importa**, não ao diretório
  corrente: `examples/modules.ae` escreve `use "../stdlib/vec2.ae";`. A
  extensão `.ae` pode ser omitida (é acrescentada). Para fontes em memória
  (REPL, `compile_source`) a base é o diretório corrente.
- **Espaço de nomes plano.** Não há prefixos nem `mod`: `vec2_add` chama-se
  `vec2_add` em todo o lado, e dois ficheiros que definam o mesmo nome dão o
  erro habitual `duplicate function` / `duplicate struct`. As importações
  são transitivas: o que `b.ae` importa também fica visível em quem importa
  `b.ae`.
- **Cada ficheiro entra uma vez** (deduplicação pelo caminho canónico):
  importar o mesmo ficheiro duas vezes, por caminhos diferentes, ou em
  ciclo (`a` → `b` → `a`) é inofensivo. `--include` / `AETHER_INCLUDE` são
  `use`s implícitos do ficheiro principal e seguem a mesma regra.
- **Sem visibilidade.** `pub` é aceite antes de `fn`, `struct` e `extern fn`
  e registado na AST (`is_pub`), mas **não é verificado**: tudo o que um
  ficheiro define é visível em quem o importa. `pub use` não existe.
- Um ficheiro só com `use` e definições (sem `main`) é uma biblioteca; o
  `main` tem de existir exatamente uma vez no programa inteiro.
- Importação que não se consegue ler é o erro `E0280 unresolved import`,
  apontando para o `use` (`cannot read X (imported from FILE:LINE)`); o
  limite de 8 MiB por ficheiro aplica-se a cada ficheiro importado.

`use` é resolvido pelo *driver* antes da análise semântica; a AST do
programa final contém os itens de todos os ficheiros, com os diagnósticos a
apontar para o ficheiro certo.

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

### Formas de literal inteiro

Decimal `255`, hexadecimal `0xFF`, binário `0b1010`, octal `0o17`; `_` pode
separar dígitos em qualquer forma (`1_000_000`, `0xFFFF_FFFF`) e também na
mantissa de um `f64` (`1_000.5`). Os prefixos são minúsculos e denotam
*valores*, não padrões de bits: `0xFFFF_FFFF` é 4294967295 e só cabe em
`i64`; para a máscara `i32` de 32 uns escreva `-1` ou `!0`. O valor tem de
caber em `i64` ("invalid integer literal" caso contrário).

## Operadores e precedência

Do mais frouxo ao mais apertado (como em Rust: bit a bit mais apertado que
comparação, deslocamento mais frouxo que aritmética):

| Nível | Operadores           | Associatividade |
|-------|----------------------|-----------------|
| 1     | `\|\|`               | esquerda        |
| 2     | `&&`                 | esquerda        |
| 3     | `== !=`              | esquerda        |
| 4     | `< <= > >=`          | esquerda        |
| 5     | `\|`                 | esquerda        |
| 6     | `^`                  | esquerda        |
| 7     | `&`                  | esquerda        |
| 8     | `<< >>`              | esquerda        |
| 9     | `+ -`                | esquerda        |
| 10    | `* / %`              | esquerda        |
| 11    | `as`                 | esquerda        |
| 12    | prefixos `- !`       | direita         |
| 13    | chamada, `[]`, `.`   | esquerda        |

Assim `1 | 2 & 3 == 3` é `(1 | (2 & 3)) == 3` e `1 << 2 + 3` é `1 << 5`.

| Operadores            | Operandos                         | Resultado        |
|-----------------------|-----------------------------------|------------------|
| `+ - * /`             | `i32,i32` / `i64,i64` / `f64,f64` | tipo do operando |
| `%`                   | `i32,i32` / `i64,i64`             | tipo do operando |
| `+`                   | `string,string`                   | `string`         |
| `== !=`               | tipos iguais exceto `unit`        | `bool`           |
| `< <= > >=`           | `i32`, `i64`, `f64`, `char`       | `bool`           |
| `&& \|\|`             | `bool,bool`                       | `bool`           |
| `& \| ^`              | `i32,i32` / `i64,i64`             | tipo do operando |
| `<< >>`               | `i32,i32` / `i64,i64`             | tipo do esquerdo |
| `-` prefixo           | `i32`, `i64`, `f64`               | tipo do operando |
| `!` prefixo           | `bool`                            | `bool` (not lógico) |
| `!` prefixo           | `i32`, `i64`                      | tipo do operando (not bit a bit) |

Aritmética exige operandos do mesmo tipo numérico. `%` não se aplica a `f64`.
`+` também concatena `string`. Comparações produzem `bool`: `i32`, `i64`,
`f64` e `char` aceitam as seis (`== != < <= > >=`); `bool` e `string`
aceitam `==` e `!=`.

`&&` e `||` fazem curto-circuito: o operando direito só é avaliado quando
é preciso.

Os operadores bit a bit só existem para inteiros do mesmo tipo (não há
`&`/`|` em `bool`: use `&&`/`||`). A quantidade de deslocamento tem o tipo
do operando esquerdo e é **mascarada** à largura do tipo antes de deslocar
(`n & 31` em `i32`, `n & 63` em `i64`): `1 << 32` é `1`, `1 << -1` é
`1 << 31`, nunca um erro. `>>` é aritmético (replica o sinal: `-16 >> 2 ==
-4`). O otimizador dobra constantes com exactamente estas regras.

Overflow de inteiros na VM é wrapping (two's complement), incluindo a
divisão (`i32::MIN / -1 == i32::MIN`, sem pânico). Divisão por zero é erro
de runtime.

### Atribuição composta

`alvo op= expr` com `op` em `+ - * / % & | ^ << >>` é açúcar sintáctico
para `alvo = alvo op expr`, com as mesmas regras de tipo e de mutabilidade
que a atribuição simples; o alvo pode ser variável `mut`, elemento de array
ou campo de struct (`a[i] += 1`, `p.x -= 2`, `s += "!"`). O lado direito é
a expressão inteira (`x *= 2 + 3` é `x = x * 5`). O alvo é avaliado duas
vezes (uma como valor, outra como posição), logo `a[f()] += 1` chama `f`
duas vezes.

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
- `yield;` — suspende uma execução com orçamento (`Vm::run_budget`); sob
  `aether run` não faz nada. Pensado para scripts que atravessam frames de
  um jogo (ver `docs/scripting.md`).

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

Índice fora do intervalo é erro de runtime. `len(a)` devolve o número de
elementos (`N`) como `i32`.

## Tuplas

```
let t: (i32, bool) = (1, true);
t.0            // 1
t.1 = false;   // campos são atribuíveis (t: mut)
let (a, b) = t;
```

Tipo `(T1, T2, ...)` e expressão `(e1, e2, ...)` com **dois ou mais**
elementos (`()` continua a ser `unit`, `(e)` é só `e`). Acesso posicional
`t.0`, `t.1`, ... (`t.0.1` acede ao elemento 1 do elemento 0). A
desestruturação `let [mut] (a, b, ...) = expr;` exige tantos nomes quantos
elementos (E0269); `_` descarta um elemento. Tuplas têm semântica de valor
como structs. `==` / `!=` comparam elemento a elemento (todos os elementos
têm de suportar `==`); `<` etc. não existem.

## Enums

```
enum Shape {
    Circle(f64),
    Rect(i32, i32),
    Empty,
}
let c = Shape::Circle(1.5);
let e = Shape::Empty;
```

Cada variante tem 0..n cargas posicionais. Construção `Enum::Variante(args)`
(a aridade e os tipos são verificados: E0267, E0269); variante sem carga
escreve-se sem parênteses. `==` / `!=` comparam a etiqueta e, se igual, a
carga elemento a elemento; `<` etc. são rejeitados (E0244). Um enum não
pode conter-se a si próprio, directa ou indirectamente (E0205); nomes de
structs e enums partilham o mesmo espaço (E0201). As cargas só são
acessíveis por `match` / `if let`.

Representação: um objecto cujo campo 0 é a etiqueta (`i32`, índice da
variante na declaração) e os campos `1..=max_carga` as ranhuras de carga
(as não usadas pela variante activa ficam `unit`).

## `match`

```
match s {
    Shape::Circle(r) => { ... }
    Shape::Rect(w, _) => { ... }
    _ => { ... }
}
```

`match` é uma **instrução** (como `if`): cada braço é `Padrão => Bloco`
(vírgula opcional entre braços) e os blocos podem `return` / `break` /
`continue`. Os braços são testados por ordem. Padrões:

- `Enum::Variante(p1, ..., pn)` com um nome (vincula a carga, imutável) ou
  `_` por posição (padrões aninhados não são suportados, E0268);
- literal `i32` / `i64` (também negativo), `bool`, `char`, `string`, para
  escrutinador do mesmo tipo (E0269; floats não são padrões);
- `nome` — vincula o escrutinador inteiro e apanha tudo;
- `_` — apanha tudo.

Exaustividade (E0270): um `match` sobre enum cobre todas as variantes ou
tem um braço `_`/nome; sobre escalares exige sempre um braço `_`/nome. Uma
variante repetida é erro (E0271). Um `match` cujos braços todos retornam
conta como caminho de retorno da função.

## `if let`

```
if let Shape::Rect(w, h) = s { ... } else { ... }
```

Açúcar para um `match` de dois braços: o padrão dado e `_ => { else }`
(bloco vazio sem `else`). Vale qualquer padrão de `match`.

## Strings

Literais `"..."` com escapes `\n \t \r \0 \\ \" \'` e `\u{XXXX}` (1 a 6
dígitos hexadecimais de um valor escalar Unicode; `\u{41}` é `'A'`). Um
`\u{...}` malformado é erro E0005 e vale U+FFFD. Os mesmos escapes valem em
literais `'c'`. Concatenação `+`.
`len(s)` devolve `i32` e conta valores escalares Unicode (chars), como a
indexação, não bytes. Indexação devolve `char`.

## Built-ins

São funções do runtime, não palavras-chave. Uma `fn` do utilizador com o
mesmo nome **sombreia** o built-in (por isso `stdlib/math.ae` pode definir
o seu próprio `abs`).

| Nome                              | Efeito / resultado                                   |
|-----------------------------------|------------------------------------------------------|
| `print(s: string)`                | escreve `s`                                          |
| `println(s: string)`              | escreve `s` e `\n`                                   |
| `print_i32(x: i32)`, `print_i64(x: i64)`, `print_f64(x: f64)`, `print_bool(b: bool)` | escreve o valor e `\n` |
| `print_char(c: char)`             | escreve o char e `\n`                                |
| `len(x: string \| [T; N]) -> i32` | chars de uma string / elementos de um array          |
| `assert(b: bool)`                 | erro de runtime quando `false`                       |
| `to_string(x: i32) -> string`     | `to_string(42)` é `"42"`                             |
| `i64_to_string(x: i64) -> string` | idem para `i64`                                      |
| `f64_to_string(x: f64) -> string` | formato de `print_f64`: `1.5` → `"1.5"`, `2.0` → `"2"` |
| `char_to_string(c: char) -> string` | string de um char                                  |
| `abs(x: i32) -> i32`              | wrapping: `abs(-2147483648)` é `-2147483648`         |
| `min(a: i32, b: i32) -> i32`, `max(a: i32, b: i32) -> i32` |                             |
| `clamp(x: i32, lo: i32, hi: i32) -> i32` | `max(lo, min(hi, x))`                         |
| `sqrt(x: f64) -> f64`, `floor(x: f64) -> f64`, `ceil(x: f64) -> f64` | IEEE-754 (`sqrt(-1.0)` é NaN) |
| `pow_i32(base: i32, exp: i32) -> i32` | potência wrapping; `exp < 0` devolve `0`         |

(O emissor LLVM de estudo só embrulha `len` para `string`.)

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

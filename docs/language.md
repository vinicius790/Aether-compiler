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
- **Sem prefixos.** Não há `mod`: `vec2_add` chama-se `vec2_add` em todo o
  lado. Um item `pub` colide com qualquer outro item do mesmo nome (o erro
  habitual `duplicate function` / `duplicate struct`), tal como dois itens
  do mesmo ficheiro. As importações são transitivas: o que `b.ae` importa
  também fica visível em quem importa `b.ae`.
- **Nomes privados por ficheiro.** Um item sem `pub` pertence ao ficheiro
  que o define: um `fn helper` privado em `a.ae` e outro em `b.ae` compilam
  ambos e cada ficheiro chama o seu (o mesmo para `struct` e `enum`). Um nome usado no ficheiro F resolve para o item de F com
  esse nome, se existir, senão para o item `pub` com esse nome, senão para o
  built-in. Internamente os itens privados dos ficheiros que não são o
  principal chamam-se `nome$N` (N = número do ficheiro); `main`, os itens
  `pub` e os do ficheiro principal mantêm o nome. Diagnósticos, `dump-ir` e
  os tipos mostram sempre o nome do fonte. `main` e `extern fn` (cujo nome é
  o símbolo do anfitrião) nunca mudam de nome: dois `main`, ou duas
  declarações do mesmo `extern fn`, colidem sempre (um `extern fn` privado
  continua invisível aos outros ficheiros, E0281).
- **Cada ficheiro entra uma vez** (deduplicação pelo caminho canónico):
  importar o mesmo ficheiro duas vezes, por caminhos diferentes, ou em
  ciclo (`a` → `b` → `a`) é inofensivo. `--include` / `AETHER_INCLUDE` são
  `use`s implícitos do ficheiro principal e seguem a mesma regra.
- **Visibilidade (`pub`).** Um item definido num ficheiro diferente do que o usa (via `use` ou `--include`) só é acessível se for `pub` (`pub fn`, `pub struct`, `pub enum`, `pub extern fn`); nomear um item privado de outro ficheiro (sem haver um item visível com esse nome) é o erro `E0281` "`NOME` is private to `FICHEIRO`", com `help: mark it `pub` in FICHEIRO`. Os itens do mesmo ficheiro são sempre acessíveis; os itens do ficheiro principal só são acessíveis a ele próprio. `pub struct` expõe o tipo; os campos são sempre públicos. `pub enum` expõe o tipo e todas as variantes (`E::A` em expressões e padrões); um `enum` sem `pub` é privado ao seu ficheiro como um `struct`. Um valor de tipo privado pode circular (ser devolvido, guardado, lido campo a campo) sem que o outro ficheiro nomeie o tipo. `pub use` não existe.
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
`f64 → inteiro` trunca para zero e **satura** nos limites do tipo (NaN dá
`0`); `i64 → i32` mantém os 32 bits baixos (wrapping); `i32 → char` fora do
domínio dos valores escalares Unicode dá U+FFFD.

### Inferência

`let x = 1;` produz `i32`. `let x: i64 = 1;` produz `i64` porque o literal
inteiro é flexível entre `i32` e `i64` quando o contexto espera um inteiro.

O tipo esperado propaga-se através do menos unário, de parênteses e dos
operadores aritméticos e de comparação; um literal inteiro nu adopta o tipo
do outro operando (`1 + a` com `a: i64` é `i64`).

Um literal negativo é um único literal: `let y: i64 = -1;` é válido e
`-2147483648` é um `i32` válido. Um literal inteiro cujo tipo resulte `i32`
e que não caiba em 32 bits é erro (E0263); em contexto `i64` o literal pode
usar os 64 bits, incluindo `-9223372036854775808` (`i64::MIN`, só em
contexto `i64`; em contexto `i32` é E0263). Só um `-` escrito directamente
antes do literal forma o literal negativo: `--9223372036854775808` nega
`i64::MIN` como literal e é E0263, `-(-5)` é uma negação em runtime.

### Formas de literal inteiro

Decimal `255`, hexadecimal `0xFF`, binário `0b1010`, octal `0o17`; `_` pode
separar dígitos em qualquer forma (`1_000_000`, `0xFFFF_FFFF`) e também na
mantissa de um `f64` (`1_000.5`). Os prefixos são minúsculos e denotam
*valores*, não padrões de bits: `0xFFFF_FFFF` é 4294967295 e só cabe em
`i64`; para a máscara `i32` de 32 uns escreva `-1` ou `!0`. O valor tem de
caber em `i64`: `9223372036854775808` sem `-` é "integer literal out of
range for i64" e uma forma mal escrita (`0x`, `0o8`) "invalid integer
literal"; a única excepção é a magnitude de `i64::MIN` imediatamente a
seguir a `-` (ver acima). Um
`f64` escreve-se `1.5`, `1_000.5`, `2e10` ou `1.5e-3` (o expoente dispensa a
parte fraccionária; sem expoente o `.` e um dígito depois dele são
obrigatórios, de modo que `1.` e `.5` não são literais).

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
pelo parser (E0101), assim como uma cadeia de mais de 10000 operadores
binários / `as` ao mesmo nível (`a + b + c + ...`: a árvore é tão funda
quanto a cadeia é longa).

## Declarações

```
let [mut] nome [: tipo] [= expr];
```

Sem inicializador, a variável começa com o valor por omissão do seu tipo
(o tipo é então obrigatório, E0231): `0` / `0` (`i64`) / `0.0` / `false` /
`""` / `'\0'` / `()`, e para agregados o mesmo recursivamente — `let a:
[i32; 3];` é `[0, 0, 0]` (`len(a) == 3`, `a[1] = 5` funciona), `let s: S;`
tem todos os campos a zero, `let t: (i32, f64);` é `(0, 0.0)`. Um `enum` não
tem valor por omissão: `let e: E;` é o erro E0232 "enum variable needs an
initializer", e também qualquer tipo que contenha um enum (um campo, um
elemento de tupla, ou um array de comprimento > 0 de enums).

Reatribuição só é permitida em bindings `mut` e em elementos de array /
campos de struct / campos de tupla obtidos por indexação, também aninhados
(`a[i][j] = v`, `o.inner.x = v`, `t.0 = v`). Esta segunda forma **não exige**
que o binding seja `mut` (nem que seja um parâmetro): como arrays, structs
e tuplas têm semântica de valor, só a própria variável vê a alteração.

## Funções

```
fn nome(p1: T1, p2: T2) -> R { ... }
extern fn nome(p1: T1) -> R;
```

Os nomes dos parâmetros são distintos (repetir um é E0274). Argumentos
são passados por valor. Structs e arrays têm semântica de valor:
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
  em `i32` (os limites têm de ser `i32`, E0238), ambos avaliados **uma só
  vez** antes do laço (reatribuir a variável usada como limite dentro do
  corpo não muda o número de iterações); a variável de iteração é
  `mut i32` e só existe no corpo do laço. O laço é `i = início; while i <
  fim { corpo; i += 1 }`: atribuir à própria variável no corpo afecta as
  iterações seguintes (`for i in 0..5 { print_i32(i); i += 2; }` imprime
  `0` e `3`)
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
posição declarada. Cada campo é nomeado exactamente uma vez (em falta é
E0253, repetido E0275). Um literal aceita acesso a campo como qualquer
outra expressão primária (`S { a: 1 }.a`). Uma struct sem campos
(`struct U {}`) tem o literal `U {}`; na cabeça de `if` / `while` /
`match` / de um intervalo `for`, fora de parênteses, `U {}` lê-se como o nome
`U` seguido de um bloco vazio (como em Rust), por isso escreve-se
`if (u == U {}) { .. }`.

## Arrays

```
let a: [i32; 4] = [1, 2, 3, 4];
a[i]
```

O índice é `i32` (outro tipo, `i64` incluído, é E0246; converta com
`as i32`). Índice fora do intervalo é erro de runtime. `len(a)` devolve o número de
elementos (`N`) como `i32`. `N` é um literal inteiro ≥ 0 (no máximo
`2147483647`, E0262; a VM limita ainda os arrays a 2^28 elementos).

**Repetição** `[expr; N]` (N literal inteiro ≥ 0): um array `[T; N]` com `N`
cópias de `expr`, que é avaliada **uma vez** (também quando `N` é 0, pelos
seus efeitos). As cópias são independentes (semântica de valor): `let mut
m = [[0; 3]; 2]; m[0][1] = 5;` não altera `m[1]`. `[0; 100000]` compila
num ciclo, não em 100000 escritas.

**Arrays vazios.** `[T; 0]` é um tipo válido em qualquer posição (variável,
parâmetro, campo, elemento). `[]` é válido quando o contexto espera um
array (`let z: [i32; 0] = [];`, como argumento ou campo de tipo `[T; 0]`);
sem contexto é E0250 "cannot infer type of empty array". `len(z) == 0` e
qualquer índice é erro de runtime.

## Tuplas

```
let t: (i32, bool) = (1, true);
t.0            // 1
t.1 = false;   // campos são atribuíveis, como elementos de array e campos de struct
let (a, b) = t;
```

Tipo `(T1, T2, ...)` e expressão `(e1, e2, ...)` com **dois ou mais**
elementos (`()` continua a ser `unit`, `(e)` é só `e`). Acesso posicional
`t.0`, `t.1`, ... (`t.0.1` acede ao elemento 1 do elemento 0); o índice é
um decimal canónico, sem zeros à esquerda (`t.01` é "invalid tuple index"). A
desestruturação `let [mut] (a, b, ...) = expr;` exige tantos nomes quantos
elementos (E0269); `_` descarta um elemento. A desestruturação pode ser
aninhada: `let (a, (b, c)) = t;` (só nomes, `_` e tuplas; qualquer outro
padrão é E0268 e um nome repetido no mesmo padrão E0274). Tuplas têm
semântica de valor como structs. `==` / `!=` comparam elemento a elemento (todos os elementos
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
let area = match s { Shape::Circle(r) => 3 * r * r, Shape::Rect(w, h) => w * h, Shape::Empty => 0 };
```

Cada braço é `Padrão => Bloco` ou `Padrão => expressão` (vírgula opcional
depois de um bloco, obrigatória entre braços de expressão; um `,` final é
aceite). Os braços são testados por ordem; o escrutinador é avaliado uma só
vez. `return`, `break` e `continue` são aceites como corpo de braço sem
`;` (`0 => return 1,`).

### Padrões

- `Enum::Variante(p1, ..., pn)` com **padrões quaisquer** por posição:
  nomes, `_`, literais, tuplas e outras variantes (`Out::W(In::A(x))`,
  `E::A(1)`, `E::P((a, 0))`);
- `(p1, p2, ...)` — tupla com dois ou mais elementos; `()` casa o valor
  `unit` e `(p)` é só `p`;
- literal `i32` / `i64` (também negativo), `bool`, `char`, `string`, para
  escrutinador do mesmo tipo (E0269); literais `f64` não são padrões (E0268);
- `nome` — vincula o valor inteiro (imutável) e apanha tudo;
- `_` — apanha tudo.

Um padrão que não corresponde ao tipo é E0269, aridade errada de variante
E0267, variante/enum desconhecidos E0266/E0265, um nome vinculado duas
vezes no mesmo padrão E0274. Os vínculos de um braço só existem nesse braço
(e sombreiam nomes exteriores, com o aviso W0232).

### Exaustividade e alcançabilidade

A verificação é por matriz de padrões (especialização recursiva, estilo
Maranget) e vê através de aninhamento: `E::A(1)` + `E::A(_)` + `E::B` é
exaustivo. Um `match` não exaustivo é E0270 e a mensagem dá um exemplo de
valor por cobrir (`pattern `E::A(_)` not covered`, `(_, false)`,
`Out::W(In::B)`). Enums, tuplas e `bool` têm assinatura finita (`true` e
`false` bastam, sem `_`); inteiros, `char` e `string` pedem sempre um braço
`_` ou nome. Um braço que nunca pode casar porque os anteriores já cobrem
tudo é o aviso **W0272** (não erro); um braço com um padrão de variante
idêntico a um anterior é o erro **E0271**. Os braços `else` de `if let` não
geram W0272. Um `match` demasiado grande para a verificação (limite de
trabalho interno) é E0270 "too large".

### `match` como expressão

`match` pode ser usado onde se espera um valor: `let a = match e { ... };`,
`return match ...;`, argumento, operando (também de `.campo`, `[i]`:
`match t { .. }.0`), condição, limite de `for`. O valor
de um braço de bloco é a sua expressão final sem `;`. Todos os braços têm o
mesmo tipo (E0273); um braço que não termina (`return` / `break` /
`continue` em todos os caminhos) tem tipo *never* e não conta. Um literal
inteiro nu num braço adopta o tipo dos restantes (`match k { 1 => 2, _ => n64 }`
é `i64`); um braço de bloco sem expressão final tem tipo `unit`.

Na posição de instrução (`match` seguido de mais código, ou como última
coisa de um bloco que não é corpo de função com valor) continua a ser uma
**instrução**: os valores dos braços são descartados e não têm de ter o
mesmo tipo. Um `match` que é a última coisa do corpo de uma função com tipo
de retorno ≠ `unit` é o valor devolvido; se todos os braços retornam,
conta como caminho de retorno (e não é preciso `return` depois dele).

## Strings

Literais `"..."` com escapes `\n \t \r \0 \\ \" \'` e `\u{XXXX}` (1 a 6
dígitos hexadecimais de um valor escalar Unicode; `\u{41}` é `'A'`). Um
`\u{...}` malformado é erro E0005 e vale U+FFFD; qualquer outra sequência
`\x` (e `\u` sem chavetas) é E0006 `unknown escape sequence`. Os mesmos
escapes valem em literais `'c'`; `''` é E0003. Um BOM UTF-8 no início do
ficheiro é ignorado. Concatenação `+`.
`len(s)` devolve `i32` e conta valores escalares Unicode (chars), como a
indexação, não bytes. Indexação (índice `i32`) devolve `char`.

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
entre variáveis: `let b = a;` copia a árvore `Value`. (No emissor LLVM os
agregados ficam inline e cada cópia é um `memcpy`: também não há aliasing.)
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

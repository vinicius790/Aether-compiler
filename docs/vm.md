# VM Aether

Máquina de registradores. Cada frame tem `nregs` slots `Value`
(`nregs` até 65535; registradores `u16`).

## Valores

`I32`, `I64`, `F64`, `Bool`, `Str`, `Char`, `Unit`, `Array`, `Object`.

## Chamadas

`Call func dest args` empurra um frame. Os primeiros `arity` registradores
recebem os argumentos. `Ret` escreve no `ret_reg` do chamador.

## Semântica de valor

Arrays e structs comportam-se como valores: `let b = a; b[0] = 9` não altera
`a`. A implementação é *copy-on-write*: `Value::Array(Rc<Vec<Value>>)` e
`Value::Object(Rc<Vec<Value>>)` (`std::rc::Rc`; a VM é single-threaded e
`Value` não é `Send`/`Sync`). `Clone` partilha o buffer em O(1); as escritas
(`StoreIdx`, `StoreField`) passam por `Rc::make_mut`, que só copia o buffer
se ainda houver outro detentor. Construa aggregates com `Value::array(vec)` /
`Value::object(vec)`; o pattern `Value::Array(xs)` continua a funcionar
(`xs: &Rc<Vec<Value>>` deref-coage a `&Vec<Value>`).

Custo (release, `bench`): passar um array de 10000 elementos a uma função
só de leitura 1000 vezes caiu de ~81 ms para ~0,6 ms em `-O0` (o resto é a
construção do literal; a chamada passou a O(1)); uma função que muta a sua
cópia paga exactamente uma cópia por chamada, na primeira escrita, em vez de
uma por `Call`/`Move` (~162 ms → ~20 ms em `-O0`, ~78 ms → ~20 ms em `-O2`).

A aritmética inteira, incluindo a divisão, usa wrapping:
`i32::MIN / -1 == i32::MIN`.

## Leitura de registradores por referência

O loop de despacho pede a instrução ao módulo por referência (`&Op`, nunca
`clone()` — `Call` carrega um `Vec` de argumentos) e lê registradores com
`reg(&frames, r) -> &Value`. Só se copia um `Value` onde a semântica de
valor o exige:

| Op                       | o que se clona                      |
|--------------------------|-------------------------------------|
| aritmética, comparações  | nada (escalares lidos por `&Value`) |
| `LoadIdx`, `LoadField`   | **só o elemento**, nunca o contentor |
| `StoreIdx`, `StoreField` | o valor armazenado; o contentor só se o `Rc` estiver partilhado |
| `Move`, argumentos de `Call` | o valor (aggregates: só o `Rc`, O(1)) |
| `Ret`                    | nada — o valor é movido do frame que termina |

Consequência: `s = s + xs[i]` num loop é O(1) por iteração; antes clonava
o array inteiro em cada leitura (O(n) por índice, O(n²) por loop).

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

## Nativos do host (`extern fn`)

Uma declaração `extern fn nome(args) -> T;` gera uma `BcFunction` com
`is_native = true` e `native_id = None`. Quando `Call` a atinge, a VM
procura `nome` no mapa de bindings:

```rust
pub type HostFn = Box<dyn FnMut(&[Value]) -> Result<Value, VmError>>;

impl Vm<'_> {
    pub fn with_host_fn(self, name: &str, f: HostFn) -> Self;
}

pub fn execute_captured_with(
    module: &BytecodeModule,
    opts: VmOptions,
    host: Vec<(String, HostFn)>,
) -> (Result<Value, VmError>, String, u64); // (resultado, stdout parcial, passos)
```

Os argumentos chegam já verificados pelo sema contra a assinatura da
`extern`. Se a função tem `dest` (devolve valor) o resultado do closure é
escrito nesse registrador; uma `extern` que devolve unit ignora-o. Sem
binding, o erro continua a ser
``extern function `nome` has no implementation in the VM``. Um `Err` do
closure aborta a execução como qualquer `VmError`.

O valor devolvido por um `extern fn` é **validado** contra o tipo de retorno
declarado (`BcFunction::ret_ty`): escalares pela etiqueta exacta (`i32` não
aceita `I64`, `Unit` nem `Str`), arrays pelo comprimento e elementos, structs
e tuplas campo a campo, enums pela etiqueta e pela carga da variante activa.
Uma discordância aborta com
``extern function `f` returned string `x` but is declared to return `i32` ``
em vez de entregar lixo ao script. Uma `extern` que devolve unit ignora o
valor. Registar o mesmo nome duas vezes mantém o último binding. O closure é
chamado exactamente uma vez por `Call` em ordem de programa a qualquer `-O`:
chamadas nunca são removidas, fundidas (CSE), reordenadas nem inlined.

Aggregates chegam ao host como `Value::Array`/`Value::Object`; o `Rc` é
transparente na leitura e devolver um array é apenas embrulhar um `Vec`:

```rust
.register("sum", |args| {
    let mut s = 0;
    if let Some(Value::Array(xs)) = args.first() {
        for x in xs.iter() { s += x.as_i32(); }   // xs: &Rc<Vec<Value>>
    }
    Ok(Value::I32(s))
})
.register("pair", |_| Ok(Value::array(vec![Value::I32(1), Value::I32(2)])))
```

Para alterar um array recebido, clone o `Value` (O(1)) e escreva via
`Rc::make_mut`; o chamador nunca vê a mutação.

`run_captured`/`execute_captured` são `execute_captured_with(m,
VmOptions::default(), vec![])`. A camada de embedding fica em
[`scripting.md`](scripting.md) (`host::Host`).

## Execução retomável (`run_budget`)

A pilha de frames vive na própria `Vm` (inicializada preguiçosamente), por
isso a execução pode ser fatiada:

```rust
pub enum Step { Finished(Value), Yielded }

impl Vm<'_> {
    pub fn run_budget(&mut self, max_steps: u64) -> Result<Step, VmError>;
    pub fn is_finished(&self) -> bool;
    pub fn run(&mut self) -> Result<Value, VmError>; // loop sobre run_budget
}
```

- `run_budget(n)` executa no máximo `n` instruções nesta chamada e devolve
  `Yielded` se o programa não terminou. O estado fica guardado: a chamada
  seguinte retoma exactamente onde parou.
- `Finished(v)` quando `main` devolve. Depois disso `is_finished()` é
  `true` e qualquer nova chamada devolve o mesmo `Finished(v)`.
- O limite total `VmOptions::max_steps` continua a contar entre chamadas
  (`steps()` é acumulado): ultrapassá-lo dá `VmError::StepLimit` como
  antes. Depois de um erro a VM fica parada; novas chamadas devolvem erro.
- `run()` é `loop { run_budget(u64::MAX) }`, logo tem a semântica
  anterior byte a byte (incluindo `StepLimit`, `StackOverflow`, contagem
  de passos).

## ISA: `Yield`

`Op::Yield` (disassembly `yield`) é um ponto de escalonamento cooperativo:
`run_budget` devolve `Yielded` logo a seguir a avançar o `pc`; `run()`
trata-o como `nop`. Conta como uma instrução para `steps()`. A instrução
`yield;` da linguagem gera-o. O otimizador nunca remove nem funde um `yield`
(DCE trata-o como efeito) e o inliner copia-o tal e qual para o caller: o
número de suspensões de um programa é igual a `-O0` e `-O2`, mesmo com
`yield` dentro de funções inlined, chamadas aninhadas e laços. O que muda
com o inlining é o número de instruções (`steps()`), logo as fronteiras de um
orçamento fixo.

### Casos limite de `run_budget`

- `run_budget(0)` não avança nada e devolve `Yielded` (inicia a VM, por isso
  um módulo sem `main` dá `MissingMain` já aqui).
- Depois de `Finished`, qualquer orçamento (0 incluído) devolve o mesmo
  `Finished(v)` sem contar passos.
- Depois de um erro a VM fica *halted*: `run`/`run_budget` devolvem
  `vm halted after a previous error` (não repetem o erro original).
- Equivalência: para qualquer orçamento `n >= 1`, valor, stdout e `steps()`
  são idênticos aos de `run()` (testado em todos os exemplos).

## Valores, conversões e formatação

- Inteiros: wrapping em tudo (`i32::MIN / -1 == i32::MIN`, `abs(MIN) == MIN`,
  `-MIN == MIN`); deslocamentos mascarados (`& 31` / `& 63`); `i64 as i32`
  trunca; divisão por zero é erro.
- `f64 as i32/i64` satura nos limites e `NaN` dá 0; `i32 as char` com valor
  que não é escalar Unicode (negativo, surrogate, `> 0x10FFFF`) dá U+FFFD.
- `f64` imprime com o `Display` do Rust: `NaN`, `inf`, `-inf`, `-0`, `2` para
  `2.0`, sem notação científica (`1e21` → `1000000000000000000000`, `1e-7` →
  `0.0000001`). O emissor LLVM de estudo formata de outro modo.
- `let x: T;` sem inicializador: `i32`/`i64`/`f64`/`bool`/`string`/`char`
  valem `0`/`0`/`0`/`false`/`""`/`'\0'`. Arrays, structs, tuplas e enums
  ficam `Unit`: ler ou escrever neles é agora erro de runtime claro
  (`cannot store into a value of type unit`), não lixo silencioso.
- `len` e a indexação de strings contam chars e são O(n) por acesso.

## Limites

- 50 milhões de instruções por execução (configurável)
- 10 mil frames de chamada; `main` conta como 1, logo `max_call_depth = L`
  admite exactamente `L` frames vivos (`StackOverflow` ao empilhar o `L+1`)
  e `max_steps = N` admite exactamente `N` instruções (a `N+1` dá
  `StepLimit`). Com inlining um `-O2` pode usar menos frames que `-O0`
- strings até 256 MiB (`MAX_STRING_BYTES`) e arrays até 2^28 elementos
- divisão por zero e índice inválido abortam com `VmError`
- chamar uma `extern fn` sem binding é `VmError` (antes
  comportava-se como `print`)
- `len` conta caracteres (valores escalares Unicode), não bytes

Num erro de runtime a CLI imprime o stdout produzido até ali, depois
`runtime error: ...`, e sai com código 2.

## Inspeção

`aether dump-bytecode` e `aether run --stats` (passos executados).

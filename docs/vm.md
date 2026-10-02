# VM Aether

Máquina de registradores. Cada frame tem `nregs` slots `Value`
(`nregs` até 65535; registradores `u16`).

## Valores

`I32`, `I64`, `F64`, `Bool`, `Str`, `Char`, `Unit`, `Array`, `Object`.

## Chamadas

`Call func dest args` empurra um frame. Os primeiros `arity` registradores
recebem os argumentos. `Ret` escreve no `ret_reg` do chamador.

## Semântica de valor

Arrays e structs copiam-se por valor (cópia profunda). A aritmética inteira,
incluindo a divisão, usa wrapping: `i32::MIN / -1 == i32::MIN`.

## Leitura de registradores por referência

O loop de despacho pede a instrução ao módulo por referência (`&Op`, nunca
`clone()` — `Call` carrega um `Vec` de argumentos) e lê registradores com
`reg(&frames, r) -> &Value`. Só se copia um `Value` onde a semântica de
valor o exige:

| Op                       | o que se clona                      |
|--------------------------|-------------------------------------|
| aritmética, comparações  | nada (escalares lidos por `&Value`) |
| `LoadIdx`, `LoadField`   | **só o elemento**, nunca o contentor |
| `StoreIdx`, `StoreField` | o valor armazenado                  |
| `Move`, argumentos de `Call` | o valor (cópia profunda)        |
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
trata-o como `nop`. Conta como uma instrução para `steps()`. Nenhum
emissor o gera ainda — a frontend irá adicionar uma instrução `yield;`.

## Limites

- 50 milhões de instruções por execução (configurável)
- 10 mil frames de chamada
- divisão por zero e índice inválido abortam com `VmError`
- chamar uma `extern fn` sem binding é `VmError` (antes
  comportava-se como `print`)
- `len` conta caracteres (valores escalares Unicode), não bytes

Num erro de runtime a CLI imprime o stdout produzido até ali, depois
`runtime error: ...`, e sai com código 2.

## Inspeção

`aether dump-bytecode` e `aether run --stats` (passos executados).

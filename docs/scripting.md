# Aether como runtime de script (catálogo §5.5, §6.7, §5.10)

Ideias tiradas do catálogo de repositórios que **cabem neste projecto**
sem o transformar noutro.

## O que o catálogo pedia e o que o Aether faz

| Item | Pedido | Neste repo |
|------|--------|------------|
| 5.5 VM de script | fonte → bytecode → VM, bindings | pipeline completo; natives em `runtime`; `extern fn` ligadas via `host::Host` |
| 5.5 debugger | stack + locals | frames na VM; inspeção via `profile` |
| 5.5 orçamento por frame | corrotinas / fatias | `Vm::run_budget` + `Op::Yield` |
| 6.7 profiler | contagem / flame | `aether profile` — calls por função + passos |
| 5.10 replay | seed + checksum | `aether digest` (FNV-1a de valor+stdout); fuzz `--seed` |
| 6.1 subset C | fold, DCE, LLVM opcional | já existia; LLVM continua textual |
| 6.6 WASM | sandbox | a VM *é* o sandbox; não há WASM |

## Comandos

```bash
aether profile examples/fib.ae -O0
aether digest examples/hello.ae
```

O digest tem de ser estável para o mesmo fonte e o mesmo `-O`. O
oráculo diferencial (`fuzz --kind diff`) é o par O0/O2 do mesmo valor;
o digest é o fingerprint para regressão externa.

## Bindings de engine (`extern fn` + `Host::register`)

O script declara a assinatura; o host fornece o corpo em Rust. O sema
verifica os tipos dos argumentos, por isso o closure pode ler `args[i]`
directamente.

```aether
// score.ae
extern fn set_score(x: i32);
extern fn rand() -> i32;

fn main() -> i32 {
    let mut total = 0;
    let mut i = 0;
    while i < 3 {
        total = total + rand();
        set_score(total);
        i = i + 1;
    }
    return total;
}
```

```rust
use aether::host::Host;
use aether::vm::Value;
use std::cell::RefCell;
use std::rc::Rc;

let scores = Rc::new(RefCell::new(Vec::new()));
let sink = scores.clone();
let mut rng = 42u32;

let result = Host::new()
    .register("set_score", move |args| {
        sink.borrow_mut().push(args[0].as_i32());
        Ok(Value::Unit)
    })
    .register("rand", move |_args| {
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        Ok(Value::I32((rng >> 16) as i32 % 6 + 1))
    })
    .eval("score.ae", std::fs::read_to_string("score.ae")?.as_str(), 2)?;

println!("total = {}  calls = {:?}", result.value, scores.borrow());
```

- `register(name, f)` aceita qualquer `FnMut(&[Value]) -> Result<Value,
  VmError> + 'static`; o closure pode guardar estado (`rng`) ou partilhar
  com o host (`Rc<RefCell<_>>`).
- Uma `extern` que devolve unit ignora o `Value` devolvido pelo closure;
  um `Err(VmError::Native(..))` aborta o script com essa mensagem.
- `Host.opts` é o `VmOptions` da execução (`max_steps`, `trace`, ...).
- Chamar uma `extern` sem binding continua a ser erro de runtime:
  ``extern function `nome` has no implementation in the VM``.

Para usar a `Vm` directamente: `Vm::new(&bc, opts).with_host_fn("rand",
Box::new(f))`, ou `vm::execute_captured_with(&bc, opts, vec![("rand".into(),
Box::new(f))])` para ter stdout capturado.

## Orçamento por frame (`run_budget`)

Um script de jogo não pode monopolizar o frame. A `Vm` guarda a pilha de
frames, por isso o loop do jogo pode dar-lhe uma fatia de instruções por
frame e retomar no seguinte:

```rust
use aether::vm::{Step, Vm, VmOptions};
use aether::{compile_source, CompileOptions};

let compiled = compile_source("ai.ae", &source, &CompileOptions { opt_level: 2, color: false });
assert!(!compiled.diags.has_errors());
let bc = compiled.bytecode.as_ref().expect("bytecode");

let mut vm = Vm::new(bc, VmOptions::default())
    .with_host_fn("set_score", Box::new(|a| { /* ... */ Ok(aether::Value::Unit) }));

loop {
    // ... input, física ...
    match vm.run_budget(5_000)? {
        Step::Yielded => {}                       // o script continua no próximo frame
        Step::Finished(v) => { println!("script terminou: {v}"); break; }
    }
    // ... render ...
}
```

`run_budget(n)` executa no máximo `n` instruções e devolve `Yielded` se
ainda há trabalho; a chamada seguinte retoma exactamente onde parou (mesmo
a meio de uma chamada recursiva). O limite global `VmOptions::max_steps`
continua a aplicar-se ao total acumulado. Quando a frontend emitir
`Op::Yield` (instrução `yield;`), o script poderá ceder o frame
voluntariamente — `run_budget` devolve `Yielded` nesse ponto e `run()`
ignora-o. Detalhes em [`vm.md`](vm.md).

## O que não entra

Hot-reload, GC, `vec2`/`entity` como tipos da linguagem. Bindings fazem-se
por `extern fn` + `Host::register`; o resto seria o repo 5.5 sozinho, não
um anexo deste compilador.

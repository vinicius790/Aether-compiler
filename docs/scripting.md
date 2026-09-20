# Aether como runtime de script (catálogo §5.5, §6.7, §5.10)

Ideias tiradas do catálogo de repositórios que **cabem neste projecto**
sem o transformar noutro.

## O que o catálogo pedia e o que o Aether faz

| Item | Pedido | Neste repo |
|------|--------|------------|
| 5.5 VM de script | fonte → bytecode → VM, bindings | pipeline completo; natives em `runtime` |
| 5.5 debugger | stack + locals | frames na VM; inspeção via `profile` |
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

## O que não entra

Hot-reload, coroutines, `vec2`/`entity`, GC, bindings de engine.
Isso seria o repo 5.5 sozinho, não um anexo deste compilador.

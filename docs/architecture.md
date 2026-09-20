# Arquitetura do compilador

## Visão geral

```
                    ┌─ assemble ─► Bytecode ─► VM
Source
  │
  ├ lexer   tokens
  ├ parser  AST
  ├ sema    HIR + Type
  ├ ir      IrModule
  ├ opt     IrModule'
  │
                    └─ emit_llvm_ir ─► .ll ─► opt/lli
```

O `driver` é o único módulo que conhece a ordem das etapas. Frontend, IR,
otimizador e backends não se importam uns aos outros além das estruturas
públicas (`Program`, `HirProgram`, `IrModule`, `BytecodeModule`).

## Sessão e diagnósticos

`Session` guarda os arquivos de origem e as tabelas de linhas.
`Diagnostics` acumula erros/avisos com `Span`. Nenhuma etapa imprime
direto em stderr — a CLI decide quando emitir.

## Frontend

- `lexer`: varredura linear, O(n). Tokens carregam lexema e posição.
- `parser`: recursive descent para itens/stmts; Pratt para expressões.
  Recuperação: sincroniza em `;`, `}` ou próximo item.
- `ast`: árvore owned, sem tipos. `dump_program` serve `dump-ast`.

## Semântica

`sema` percorre a AST duas vezes logicamente: coleta de itens (structs,
assinaturas, built-ins) e verificação dos corpos. A tabela de símbolos é
uma pilha de hash maps. O produto é a HIR, já anotada com `Type`.

## IR

Três endereços, blocos básicos, terminadores explícitos (`Jump`, `Branch`,
`Return`). Registradores virtuais estáveis (não SSA). Isso simplifica o
abaixamento para a VM (1 registrador IR = 1 registrador VM).

## Otimizador

Pipeline funcional sobre `IrModule`. Cada pass devolve estatísticas.
Nível 0: identidade. Nível 1: fold + copy-prop + DCE. Nível 2: + algébrico,
const-prop, simplificação de CFG, segunda rodada de fold/DCE.

## Backends

A VM é o backend de execução. LLVM é um emissor de texto para estudo e
para `opt`/`lli` quando disponíveis. Os dois partem da mesma IR.

## Runtime

Não há runtime ligado ao compilador. Nativos (`print_*`, `len`, `assert`)
são implementados na VM e reimplementados como wrappers C na emissão LLVM.

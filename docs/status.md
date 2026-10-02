# Estado do projeto

Última atualização: 2026-10-02

## Implementado

- [x] Especificação da linguagem Aether (docs/language.md)
- [x] Gramática formal (docs/grammar.md)
- [x] Lexer com posições, comentários, literais e diagnósticos
- [x] Parser recursive-descent + Pratt
- [x] AST independente de backend
- [x] Análise semântica: nomes, escopos, tipos, retornos, laços
- [x] Sistema de tipos documentado
- [x] IR de três endereços com blocos básicos
- [x] Otimizador com métricas antes/depois
- [x] Backend bytecode (VM de registradores)
- [x] Backend LLVM IR textual
- [x] Runtime mínimo via nativos da VM
- [x] CLI (`check`, `run`, `compile`, dumps, `optimize`, `repl`, `benchmark`)
- [x] REPL
- [x] Diagnósticos com trecho de origem
- [x] Testes unitários e end-to-end
- [x] Exemplos e benchmarks
- [x] Documentação de arquitetura
- [x] Fuzzing de propriedade (lexer, parser, pipeline, gerador, diferencial `-O0`/`-O2`)
- [x] Mutação estrutural + crossover + shrink (`src/fuzz/mutate.rs`, docs/fuzzers-rust.md)
- [x] Mutação aspect-preserving sobre sketch tipada (`src/fuzz/sketch.rs`)
- [x] Geração directa de IR + decoy blocks (`src/fuzz/mir.rs`, docs/aspect-mir.md)
- [x] Decoys alargados: const true/false, predicados opacos, cadeias
- [x] Greybox in-tree (cobertura de arestas na VM, corpus + energia) (`src/fuzz/greybox.rs`, docs/greybox.md)
- [x] Fold de predicados opacos (`x-x`, `x==x`, `x*0`)
- [x] Fuzzer de formato (fita EBNF, `--kind format`)
- [x] Verificador de IR (`src/ir/verify.rs`, `aether verify`)
- [x] Export DOT do CFG (`src/ir/cfg.rs`, `aether cfg`)
- [x] Pretty-printer (`src/pretty.rs`, `aether fmt`)
- [x] stdlib de referência (`stdlib/math.ae`)
- [x] Inlining de folhas de um bloco, CSE local, DCE por liveness; `profile`, `digest`, `stats`, `dump-hir`, `dump-liveness`
- [x] Emissor LLVM com `alloca` por registrador da IR (LLVM 18 IR válido, pronto para `mem2reg`)
- [x] Registradores `u16`, opcodes `Cmp` / `RemI64` / `NegI64` / conversões em falta; combinação sem opcode = erro E0300
- [x] Semântica de valor para arrays/structs, divisão com wrapping, curto-circuito `&&` / `||`, `len` em caracteres
- [x] Fuzzer tipado de agregados (`--kind agg`) e `tests/regressions.rs` (R1–R22)

## Limitações conhecidas

- Inferência de tipos apenas em `let` a partir do inicializador (não Hindley–Milner).
- Funções de primeira classe / closures: não implementadas.
- Módulos / `import`: não implementados; um arquivo = um programa.
- Strings são imutáveis e concatenáveis; não há fatiamento.
- Arrays têm tamanho fixo conhecido em tempo de compilação.
- A IR não é SSA. O emissor LLVM aloca um `alloca` por registrador da IR
  (`load`/`store`), pronto para `mem2reg`; **não é um gerador LLVM de
  produção**. A VM é o backend de execução e o contrato.
- LLVM: agregados vivem na stack e copiar um array/struct copia o ponteiro
  (aliasing, ao contrário da semântica de valor da VM); sem verificação de
  limites; concatenação de strings não suportada (aborta); `print_f64` usa `%g`.
- Não há GC: arrays e structs vivem nos registradores da VM (árvores `Value`,
  cópia profunda ao atribuir).
- Inlining só de funções folha de um bloco; sem SSA nem alocação de
  registradores global.
- Bytecode e ISA instáveis entre versões.
- CI executa `cargo test` (incluindo as propriedades); não publica artefatos.

## Decisões

- Linguagem própria em vez de subconjunto de C: especificação fechada, sem
  a dívida de compatibilidade com o pré-processador e UB de C.
- VM de registradores em vez de só LLVM: executável sem libLLVM, testes
  diferenciais possíveis, bytecode inspecionável.
- Frontend compartilhado + dois backends, sem um terceiro.
- Dependências: nenhuma além da std. Lexer, parser, IR, VM e fuzzer são código próprio.

## Roadmap

Feito em 0.2.2: stack slots (`alloca`) no LLVM prontos para `mem2reg`;
inlining de folhas no otimizador próprio; gerador estendido a structs,
arrays e strings (`--kind agg`).

Também em 0.2.2: operadores compostos e bitwise, literais hex/bin/oct,
`\u{...}`, 13 built-ins novos, `len` em arrays, `yield` + `Vm::run_budget`,
natives do host (`Host::register`), `--include`/prelúdio, `bench`, REPL com
estado, compactação de registradores, otimizador em ponto fixo com remoção
de funções mortas.

1. Módulos simples (`mod` / `use`).
2. Relatórios de cobertura LLVM / `cargo-fuzz` sobre a IR (o greybox da VM já existe).

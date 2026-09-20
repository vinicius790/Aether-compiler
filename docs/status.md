# Estado do projeto

Última atualização: 2026-09-19

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

## Limitações conhecidas

- Inferência de tipos apenas em `let` a partir do inicializador (não Hindley–Milner).
- Funções de primeira classe / closures: não implementadas.
- Módulos / `import`: não implementados; um arquivo = um programa.
- Strings são imutáveis e concatenáveis; não há fatiamento.
- Arrays têm tamanho fixo conhecido em tempo de compilação.
- A IR não é SSA. O emissor LLVM trata registradores como valores SSA e
  usa `add x, 0` como cópia — válido para muitos programas após folding,
  mas **não é um gerador LLVM de produção**. A VM é o backend de execução.
- `Move` na LLVM assume `i32` na cópia crua; programas com `f64`/`i64`
  devem ser executados na VM.
- Não há GC: arrays e structs vivem nos registradores da VM (árvores `Value`).
- Fuzzing cobre gramática i32/bool e entradas lixo; structs/arrays/strings
  aleatórios entram só via mutação e corpus, não via gerador tipado.
- CI executa `cargo test` (incluindo as propriedades); não publica artefatos.

## Decisões

- Linguagem própria em vez de subconjunto de C: especificação fechada, sem
  a dívida de compatibilidade com o pré-processador e UB de C.
- VM de registradores em vez de só LLVM: executável sem libLLVM, testes
  diferenciais possíveis, bytecode inspecionável.
- Frontend compartilhado + dois backends, sem um terceiro.
- Dependências: nenhuma além da std. Lexer, parser, IR, VM e fuzzer são código próprio.

## Roadmap

1. Alocar locais da IR em stack slots no LLVM e correr `mem2reg`.
2. Inlining de funções pequenas no otimizador próprio.
3. Estender o gerador a structs, arrays e strings.
4. Módulos simples (`mod` / `use`).
5. Relatórios de cobertura LLVM / `cargo-fuzz` sobre a IR (o greybox da VM já existe).

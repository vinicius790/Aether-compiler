# Notas extraídas da pasta Compiladores (Drive)

Fonte: *leitura* da pasta
`Biblioteca de Alexandria/TI/Compiladores/` no Google Drive.
**Nenhum ficheiro do Drive foi alterado, movido ou reenviado.**
**Nenhum PDF ou livro foi copiado para este repositório** (direitos de autor).

Este documento é um mapa *original*: o que cada peça da pasta ensina e o que
o Aether 0.2 já faz / passa a fazer / não fará.

## Inventário lido

| Item no Drive | Tipo | Uso no Aether |
|---------------|------|----------------|
| Stanford CS143, aulas 01–17 | PDF | Arquitectura do curso = arquitectura deste repo |
| *Essentials of Compilation* (Siek) | PDF | Nanpasses, RCO, explicate-control, alocação |
| *Crafting Interpreters* (Nystrom) | ZIP livro+código | VM de bytecode vs árvore; não copiado |
| `awesome-compilers.zip` | lista de ligações | Bibliografia curada abaixo |
| `write-you-a-programming-language` | ZIP pequeno | Índice de tutoriais; não copiado |

## Correspondência CS143 → Aether

O curso de Stanford parte o compilador em PA1–PA4. O Aether já está
partido do mesmo modo, com comandos de inspeção em cada fase.

| PA / aula | Tema CS143 | Onde está no Aether | Como verificar |
|-----------|------------|---------------------|----------------|
| PA1 / aulas 3–4 | Lexer, DFA, tokens | `src/lexer.rs` | `aether dump-tokens` |
| PA2 / aulas 5–8 | Parser (aqui: rec. descent, não LALR) | `src/parser.rs` | `aether dump-ast` |
| PA3 / aulas 9–10 | Semântica, scope estático, tipos | `src/sema/`, `src/ty.rs` | `aether check` |
| aula 11 | Ambiente de execução, ativações | VM de registradores, sem heap | `docs/vm.md` |
| aula 12 | Geração (CS143: stack+$a0 MIPS) | Bytecode de registradores | `aether dump-bytecode` |
| aula 13 | Semântica operacional | Oráculo: valor da VM | `aether run` |
| aula 14 | IR 3-endereços, bloco básico, opt local | `src/ir/`, `src/opt/` | `aether dump-ir`, `optimize` |
| aula 15 | Opt global, dataflow, liveness | **ainda local**; ver limitações | — |
| aula 16 | Alocação de registradores | VM tem regs ilimitados | — |
| aula 17 | GC | Fora de âmbito 0.2 | — |

Aula 01: *correctness over performance*. É a regra do `fuzz --kind diff`.

## O que a aula 14 justificou neste código

Otimizações **locais** (um bloco básico):

- fold de constantes
- identidades algébricas (`x+0`, `x*1`, `x*0`, `x-x`)
- **CSE / value numbering local** (`pass_local_cse`) — acrescentado a
  partir desta leitura: `t1 = p+1; t2 = p+1` vira `t2 = t1` no mesmo bloco

O Aether **não** implementa dataflow global (aula 15) porque a IR não é
SSA e um join sem φ mente. A lição extraída da aula 15 é a mesma que o
teste `does_not_fold_across_reassignment` já impõe: *é sempre seguro
dizer “não sei”*.

## O que o Siek justificou (sem copiar o livro)

Siek organiza o compilador como **nanpasses** testáveis. O driver Aether
já é essa lista: lexer → parse → sema → emit_ir → optimize → assemble.
A passagem “Remove Complex Operands” do livro é, no Aether, o próprio
lowering para três endereços. “Explicate control” é a descida de `if` /
`while` para blocos + `Branch`. Alocação por coloração (cap. 4) fica
de fora: a VM não tem mapa de registradores de máquina.

## Crafting Interpreters

Nystrom distingue *tree-walk* de *VM de bytecode*. O Aether é o segundo
(depois de sema+IR), não o primeiro. Nada do código Java/C do livro
entra neste repo.

## awesome-compilers (só o mapa, não o dump)

A lista pública agrupa: livros, papers, cursos, ferramentas por
linguagem, VMs. Para este projecto as entradas relevantes já estão
citadas em `docs/fuzzers-rust.md` (LibAFL, cargo-fuzz) e aqui (CS143,
Siek, Nystrom). Não se vendorize a lista.

## Código acrescentado a partir da pasta (sem copiar livros)

- `src/opt/liveness.rs` e `aether dump-liveness` — liveness para trás no CFG (CS143 aula 15), conservadora.
- `src/opt/inline.rs` — inlining de folhas (um bloco, sem calls, ≤12 insts).
- `aether dump-hir` — HIR tipada.
- `examples/cse.ae`, `examples/inline.ae`.

A coloração de grafos (aula 16 / Siek cap. 4) continua de fora: a VM não tem mapa de registradores de máquina.

## O que **não** foi extraído para código

- Slides ou capítulos inteiros
- Cool, MIPS, SPIM, GC, fechos, gradual typing
- Earley / LALR(1) — o parser Aether é descida recursiva de propósito

## Como repetir a leitura sem tocar no Drive

Só leitura. Qualquer alteração futura à pasta Compiladores/ é da
responsabilidade do dono do Drive, não deste repositório.

# Biblioteca de referência

Aether 0.2 não tem `mod` / `use`. Os ficheiros em `stdlib/` são
**programas autónomos** para copiar rotinas, não um crate ligado.

| Ficheiro | Rotinas |
|----------|---------|
| `math.ae` | abs, min, max, clamp |
| `cmp.ae` | abs, sign |
| `loops.ae` | sum_to |
| `bits.ae` | band em 0/1 |

`examples/game_score.ae` é o exemplo de script de gameplay (§5.5):
pontuação e combo, sem engine por baixo.

Embedding em Rust: `aether::host::eval` / `profile_source`.

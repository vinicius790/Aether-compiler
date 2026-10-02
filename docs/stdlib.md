# Biblioteca de referência

Aether 0.2 não tem `mod` / `use`. Os ficheiros em `stdlib/` são de dois
tipos: **programas autónomos** para copiar rotinas, e o **prelúdio**, feito
para ser incluído com `--include`.

| Ficheiro | Rotinas | Uso |
|----------|---------|-----|
| `math.ae` | abs, min, max, clamp (versões de referência; são built-ins) | `aether run stdlib/math.ae` |
| `cmp.ae` | abs, sign | programa autónomo |
| `loops.ae` | sum_to | programa autónomo |
| `bits.ae` | band em 0/1 | programa autónomo |
| `prelude.ae` | lerp, sign, is_even, gcd, clamp_f64, wrap_index, sum_to | `--include` (não tem `main`) |

## `stdlib/prelude.ae`

```bash
aether run game.ae --include stdlib/prelude.ae
AETHER_INCLUDE=stdlib/prelude.ae aether run game.ae
```

| Função | Assinatura | Nota |
|--------|------------|------|
| `lerp` | `(a: f64, b: f64, t: f64) -> f64` | `a + (b - a) * t` |
| `sign` | `(x: i32) -> i32` | -1, 0 ou 1 |
| `is_even` | `(x: i32) -> bool` | |
| `gcd` | `(a: i32, b: i32) -> i32` | Euclides; nunca negativo |
| `clamp_f64` | `(x: f64, lo: f64, hi: f64) -> f64` | |
| `wrap_index` | `(i: i32, n: i32) -> i32` | módulo para `0..n` (`-1` → `n - 1`) |
| `sum_to` | `(n: i32) -> i32` | `0 + 1 + … + (n - 1)` |

`abs`, `min`, `max` e `clamp` não estão no prelúdio porque são built-ins;
definir uma função com um destes nomes é um erro. `aether check
stdlib/prelude.ae` sozinho falha com “missing entry point” — é esse o
objetivo: o `main` vem do programa que o inclui.

`examples/game_score.ae` é o exemplo de script de gameplay (§5.5):
pontuação e combo, sem engine por baixo.

Embedding em Rust: `aether::host::eval` / `profile_source`;
`aether::driver::compile_files` / `compile_sources` para vários ficheiros e
`run_compiled_with(&mut c, VmOptions { max_steps, .. })` para limites.

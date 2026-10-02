# Biblioteca de referência

Os ficheiros em `stdlib/` são de dois tipos: **programas autónomos** para
copiar rotinas, e **ficheiros importáveis** (sem `main`), para trazer com
`use "…";` ou `--include`.

| Ficheiro | Rotinas | Uso |
|----------|---------|-----|
| `math.ae` | abs, min, max, clamp (versões de referência; são built-ins) | `aether run stdlib/math.ae` |
| `cmp.ae` | abs, sign | programa autónomo |
| `loops.ae` | sum_to | programa autónomo |
| `bits.ae` | band em 0/1 | programa autónomo |
| `prelude.ae` | lerp, sign, is_even, gcd, clamp_f64, wrap_index, sum_to | importável |
| `vec2.ae` | `Vec2`, vec2_new, vec2_add, vec2_sub, vec2_scale, vec2_dot, vec2_length | importável |
| `rng.ae` | rng_next, rng_range (xorshift32 em `i32`) | importável |

## Ficheiros importáveis

```
use "stdlib/prelude.ae";     // a partir da raiz do repositório
use "../stdlib/vec2.ae";     // a partir de examples/ (caminho relativo ao ficheiro)
```

| Ficheiro | Função | Assinatura | Nota |
|----------|--------|------------|------|
| `vec2.ae` | `Vec2` | `struct { x: f64, y: f64 }` | |
| | `vec2_new` | `(x: f64, y: f64) -> Vec2` | |
| | `vec2_add` / `vec2_sub` | `(a: Vec2, b: Vec2) -> Vec2` | |
| | `vec2_scale` | `(v: Vec2, k: f64) -> Vec2` | |
| | `vec2_dot` | `(a: Vec2, b: Vec2) -> f64` | |
| | `vec2_length` | `(v: Vec2) -> f64` | `sqrt(dot(v, v))` |
| `rng.ae` | `rng_next` | `(state: i32) -> i32` | próximo estado xorshift32 (estado 0 vira 1) |
| | `rng_range` | `(state: i32, lo: i32, hi: i32) -> i32` | mapeia um estado para `lo..hi`; `lo` se `hi <= lo` |

O estado do gerador é um `i32` que o chamador transporta: `s = rng_next(s);
let d = rng_range(s, 1, 7);`. `examples/modules.ae` usa `vec2.ae` e
`rng.ae`; `aether run examples/modules.ae` funciona a partir da raiz.

## `stdlib/prelude.ae`

```bash
aether run game.ae --include stdlib/prelude.ae
AETHER_INCLUDE=stdlib/prelude.ae aether run game.ae
# ou, dentro de game.ae (caminho relativo a game.ae):
#     use "stdlib/prelude.ae";
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

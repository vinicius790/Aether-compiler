//! Runtime contract.
//!
//! The compiler does not link a separate runtime library. Native functions
//! are implemented twice:
//!
//! * in `crate::vm` for the register machine;
//! * as LLVM `printf` wrappers in `crate::backend::llvm`.
//!
//! Adding a built-in requires updating: `sema` (signature), `bytecode`
//! (native id table), `vm` (`call_native`), and optionally the LLVM wrappers.
//!
//! The index in `NATIVES` is the native id. Signatures and output formats:
//!
//! | id | name             | signature                        | effect / result (Rust `Display`) |
//! |----|------------------|----------------------------------|----------------------------------|
//! | 0  | `print`          | `(string)`                       | writes the string, no newline    |
//! | 1  | `println`        | `(string)`                       | writes the string + `\n`         |
//! | 2  | `print_i32`      | `(i32)`                          | `{}` + `\n`                      |
//! | 3  | `print_i64`      | `(i64)`                          | `{}` + `\n`                      |
//! | 4  | `print_f64`      | `(f64)`                          | `{}` + `\n`                      |
//! | 5  | `print_bool`     | `(bool)`                         | `true`/`false` + `\n`            |
//! | 6  | `len`            | `(string | [T; N]) -> i32`       | chars / elements                 |
//! | 7  | `assert`         | `(bool)`                         | runtime error when false         |
//! | 8  | `print_char`     | `(char)`                         | the char + `\n`                  |
//! | 9  | `to_string`      | `(i32) -> string`                | `42` → `"42"`                    |
//! | 10 | `i64_to_string`  | `(i64) -> string`                | `{}`                             |
//! | 11 | `f64_to_string`  | `(f64) -> string`                | `1.5` → `"1.5"`, `2.0` → `"2"`   |
//! | 12 | `char_to_string` | `(char) -> string`               | one-char string                  |
//! | 13 | `abs`            | `(i32) -> i32`                   | wrapping (`abs(i32::MIN)` = MIN) |
//! | 14 | `min`            | `(i32, i32) -> i32`              |                                  |
//! | 15 | `max`            | `(i32, i32) -> i32`              |                                  |
//! | 16 | `clamp`          | `(i32, i32, i32) -> i32`         | `max(lo, min(hi, x))`            |
//! | 17 | `sqrt`           | `(f64) -> f64`                   | `f64::sqrt`                      |
//! | 18 | `floor`          | `(f64) -> f64`                   | `f64::floor`                     |
//! | 19 | `ceil`           | `(f64) -> f64`                   | `f64::ceil`                      |
//! | 20 | `pow_i32`        | `(i32, i32) -> i32`              | wrapping; `exp < 0` → 0          |
//!
//! A user `fn` with the same name shadows the built-in (sema registers user
//! functions first).

pub const NATIVES: &[&str] = &[
    "print",
    "println",
    "print_i32",
    "print_i64",
    "print_f64",
    "print_bool",
    "len",
    "assert",
    "print_char",
    "to_string",
    "i64_to_string",
    "f64_to_string",
    "char_to_string",
    "abs",
    "min",
    "max",
    "clamp",
    "sqrt",
    "floor",
    "ceil",
    "pow_i32",
];

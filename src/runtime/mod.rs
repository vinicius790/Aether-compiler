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

pub const NATIVES: &[&str] = &[
    "print",
    "println",
    "print_i32",
    "print_i64",
    "print_f64",
    "print_bool",
    "len",
    "assert",
];

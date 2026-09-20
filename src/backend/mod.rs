//! Backends: custom bytecode and textual LLVM IR.

pub mod bytecode;
pub mod llvm;

pub use bytecode::{assemble, BytecodeModule, Op};
pub use llvm::emit_llvm_ir;

//! Structural verifier for Aether IR.
//!
//! Independent of the fuzzer: the driver and `aether verify` use the same
//! checks that `--kind mir` runs on generated modules.

use super::{IrModule, Terminator};
use std::collections::HashSet;

pub fn verify_module(m: &IrModule) -> Result<(), String> {
    if !m.functions.iter().any(|f| f.name == "main") {
        return Err("missing main".into());
    }
    for f in &m.functions {
        if f.blocks.is_empty() {
            return Err(format!("function `{}` has no blocks", f.name));
        }
        let ids: HashSet<u32> = f.blocks.iter().map(|b| b.id.0).collect();
        if ids.len() != f.blocks.len() {
            return Err(format!("function `{}` has duplicate block ids", f.name));
        }
        for bb in &f.blocks {
            for inst in &bb.insts {
                for r in inst.uses() {
                    if r.0 >= f.reg_count {
                        return Err(format!("`{}` uses %{} >= r{}", f.name, r.0, f.reg_count));
                    }
                }
                if let Some(d) = inst.dest_reg() {
                    if d.0 >= f.reg_count {
                        return Err(format!("`{}` dest %{} >= r{}", f.name, d.0, f.reg_count));
                    }
                }
            }
            match &bb.term {
                Terminator::Jump { target } if !ids.contains(&target.0) => {
                    return Err(format!("`{}` jump to missing bb{}", f.name, target.0));
                }
                Terminator::Branch {
                    cond,
                    then_bb,
                    else_bb,
                } => {
                    if cond.0 >= f.reg_count {
                        return Err(format!("`{}` branch cond out of range", f.name));
                    }
                    if !ids.contains(&then_bb.0) || !ids.contains(&else_bb.0) {
                        return Err(format!("`{}` branch to missing block", f.name));
                    }
                }
                Terminator::Return { value: Some(r) } if r.0 >= f.reg_count => {
                    return Err(format!("`{}` return out of range", f.name));
                }
                Terminator::Unreachable => {
                    return Err(format!("`{}` bb{} still Unreachable", f.name, bb.id.0));
                }
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::emit_ir;
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use crate::sema::analyze;
    use crate::span::FileId;

    #[test]
    fn accepts_lowered_main() {
        let src = "fn main() -> i32 { return 1 + 2; }";
        let (toks, _) = tokenize(FileId(0), src);
        let (prog, _) = parse(toks);
        let (hir, _) = analyze(&prog);
        let ir = emit_ir(&hir.unwrap());
        verify_module(&ir).expect("verify");
    }
}

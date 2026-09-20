//! Extra IR pretty-printers and statistics.

use super::{Inst, IrModule};

#[derive(Debug, Default, Clone)]
pub struct IrStats {
    pub functions: usize,
    pub blocks: usize,
    pub instructions: usize,
    pub calls: usize,
    pub constants: usize,
}

pub fn stats(module: &IrModule) -> IrStats {
    let mut s = IrStats::default();
    s.functions = module.functions.len();
    for f in &module.functions {
        s.blocks += f.blocks.len();
        for bb in &f.blocks {
            s.instructions += bb.insts.len() + 1;
            for inst in &bb.insts {
                match inst {
                    Inst::Call { .. } => s.calls += 1,
                    Inst::LoadConst { .. } => s.constants += 1,
                    _ => {}
                }
            }
        }
    }
    s
}

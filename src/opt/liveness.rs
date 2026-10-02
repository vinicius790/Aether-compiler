//! Backward liveness on the CFG (CS143 lecture 15).
//!
//! Conservative on a non-SSA IR: a register is live if any successor
//! might read the current definition. The analysis never claims a value
//! is dead when a join could still see it.

use crate::ir::{IrFunction, IrModule, Terminator};
use std::collections::{HashMap, HashSet, VecDeque};

pub type LiveSet = HashSet<u32>;

#[derive(Debug, Clone, Default)]
pub struct Liveness {
    pub live_in: HashMap<u32, LiveSet>,
    pub live_out: HashMap<u32, LiveSet>,
}

impl Liveness {
    pub fn live_in_block(&self, id: u32) -> LiveSet {
        self.live_in.get(&id).cloned().unwrap_or_default()
    }
}

pub fn analyze_function(f: &IrFunction) -> Liveness {
    let mut live_in: HashMap<u32, LiveSet> = HashMap::new();
    let mut live_out: HashMap<u32, LiveSet> = HashMap::new();
    let mut index: HashMap<u32, usize> = HashMap::new();
    for (i, bb) in f.blocks.iter().enumerate() {
        live_in.insert(bb.id.0, HashSet::new());
        live_out.insert(bb.id.0, HashSet::new());
        index.insert(bb.id.0, i);
    }
    // predecessors, to requeue only the blocks a change can affect
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); f.blocks.len()];
    for (i, bb) in f.blocks.iter().enumerate() {
        let succs: Vec<u32> = match &bb.term {
            Terminator::Jump { target } => vec![target.0],
            Terminator::Branch {
                then_bb, else_bb, ..
            } => vec![then_bb.0, else_bb.0],
            _ => Vec::new(),
        };
        for t in succs {
            if let Some(&ti) = index.get(&t) {
                preds[ti].push(i);
            }
        }
    }

    // Worklist to the fixpoint. The sets only grow, so it terminates; a fixed
    // cap on rounds would under-approximate liveness on deep nesting (a use
    // more than `cap` blocks away is missed) and DCE / register allocation
    // would then clobber live values.
    let mut queue: VecDeque<usize> = (0..f.blocks.len()).rev().collect();
    let mut queued = vec![true; f.blocks.len()];
    while let Some(i) = queue.pop_front() {
        queued[i] = false;
        let bb = &f.blocks[i];
        let mut out = HashSet::new();
        match &bb.term {
            Terminator::Jump { target } => {
                if let Some(s) = live_in.get(&target.0) {
                    out.extend(s.iter().copied());
                }
            }
            Terminator::Branch {
                cond,
                then_bb,
                else_bb,
            } => {
                out.insert(cond.0);
                if let Some(s) = live_in.get(&then_bb.0) {
                    out.extend(s.iter().copied());
                }
                if let Some(s) = live_in.get(&else_bb.0) {
                    out.extend(s.iter().copied());
                }
            }
            Terminator::Return { value: Some(r) } => {
                out.insert(r.0);
            }
            _ => {}
        }

        // live_before = (live_after − def) ∪ uses: kill the destination
        // first so `r = call f(r)` keeps `r` live on entry.
        let mut live = out.clone();
        for inst in bb.insts.iter().rev() {
            if let Some(d) = inst.dest_reg() {
                live.remove(&d.0);
            }
            for u in inst.uses() {
                live.insert(u.0);
            }
        }

        let id = bb.id.0;
        live_out.insert(id, out);
        if live_in.get(&id) != Some(&live) {
            live_in.insert(id, live);
            for &p in &preds[i] {
                if !queued[p] {
                    queued[p] = true;
                    queue.push_back(p);
                }
            }
        }
    }
    Liveness { live_in, live_out }
}

pub fn analyze_module(m: &IrModule) -> Vec<(String, Liveness)> {
    m.functions
        .iter()
        .map(|f| (f.name.clone(), analyze_function(f)))
        .collect()
}

pub fn render(m: &IrModule) -> String {
    let mut s = String::new();
    for (name, lv) in analyze_module(m) {
        s.push_str(&format!("function {name}\n"));
        let f = m.functions.iter().find(|f| f.name == name).unwrap();
        for bb in &f.blocks {
            let inn = lv.live_in_block(bb.id.0);
            let mut regs: Vec<_> = inn.iter().copied().collect();
            regs.sort_unstable();
            s.push_str(&format!("  {} live-in: {:?}\n", bb.id, regs));
        }
    }
    s
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
    fn return_value_is_live() {
        let src = "fn main() -> i32 { return 1 + 2; }";
        let (toks, _) = tokenize(FileId(0), src);
        let (prog, _) = parse(toks);
        let (hir, _) = analyze(&prog);
        let ir = emit_ir(&hir.unwrap());
        let lv = analyze_function(&ir.functions[0]);
        assert!(!lv.live_out.is_empty());
    }
}

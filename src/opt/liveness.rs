//! Backward liveness on the CFG (CS143 lecture 15).
//!
//! Conservative on a non-SSA IR: a register is live if any successor
//! might read the current definition. The analysis never claims a value
//! is dead when a join could still see it.

use crate::ir::{IrFunction, IrModule, Terminator};
use std::collections::{HashMap, HashSet};

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

fn successors(t: &Terminator) -> Vec<u32> {
    match t {
        Terminator::Jump { target } => vec![target.0],
        Terminator::Branch { then_bb, else_bb, .. } => vec![then_bb.0, else_bb.0],
        Terminator::Return { .. } | Terminator::Unreachable => Vec::new(),
    }
}

/// Block indices in CFG postorder from the entry (successors before
/// predecessors), then every block the DFS did not reach. Sweeping a
/// backward problem in this order converges in a few sweeps whatever the
/// layout (lowering often lays a chain out against its control flow, which
/// made a plain reverse-layout sweep quadratic).
pub(crate) fn postorder(f: &IrFunction) -> Vec<usize> {
    let index: HashMap<u32, usize> = f.blocks.iter().enumerate().map(|(i, b)| (b.id.0, i)).collect();
    let succ: Vec<Vec<usize>> = f
        .blocks
        .iter()
        .map(|b| successors(&b.term).into_iter().filter_map(|s| index.get(&s).copied()).collect())
        .collect();
    let n = f.blocks.len();
    let mut seen = vec![false; n];
    let mut order = Vec::with_capacity(n);
    for root in 0..n {
        if seen[root] {
            continue;
        }
        seen[root] = true;
        let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
        while let Some(top) = stack.last_mut() {
            let (b, k) = *top;
            if k < succ[b].len() {
                top.1 += 1;
                let s = succ[b][k];
                if !seen[s] {
                    seen[s] = true;
                    stack.push((s, 0));
                }
            } else {
                order.push(b);
                stack.pop();
            }
        }
    }
    order
}

pub fn analyze_function(f: &IrFunction) -> Liveness {
    // Per block: `gen` = registers read before any write in the block,
    // `kill` = registers written. live_in = gen ∪ (live_out − kill); the
    // backward scan kills the destination before adding the uses, so
    // `r = call f(r)` keeps `r` live on entry.
    let n = f.blocks.len();
    let mut gen: Vec<LiveSet> = Vec::with_capacity(n);
    let mut kill: Vec<LiveSet> = Vec::with_capacity(n);
    for bb in &f.blocks {
        let mut g = LiveSet::new();
        let mut k = LiveSet::new();
        if let Terminator::Branch { cond, .. } = &bb.term {
            g.insert(cond.0);
        }
        if let Terminator::Return { value: Some(r) } = &bb.term {
            g.insert(r.0);
        }
        for inst in bb.insts.iter().rev() {
            if let Some(d) = inst.dest_reg() {
                g.remove(&d.0);
                k.insert(d.0);
            }
            for u in inst.uses() {
                g.insert(u.0);
            }
        }
        gen.push(g);
        kill.push(k);
    }
    let index: HashMap<u32, usize> = f.blocks.iter().enumerate().map(|(i, b)| (b.id.0, i)).collect();
    let succ: Vec<Vec<usize>> = f
        .blocks
        .iter()
        .map(|b| successors(&b.term).into_iter().filter_map(|s| index.get(&s).copied()).collect())
        .collect();
    let mut ins: Vec<LiveSet> = vec![LiveSet::new(); n];
    // A block's own branch condition / returned register counts as live-out
    // (the terminator reads it after every instruction of the block).
    let mut outs: Vec<LiveSet> = f
        .blocks
        .iter()
        .map(|bb| {
            let mut o = LiveSet::new();
            match &bb.term {
                Terminator::Branch { cond, .. } => {
                    o.insert(cond.0);
                }
                Terminator::Return { value: Some(r) } => {
                    o.insert(r.0);
                }
                _ => {}
            }
            o
        })
        .collect();
    let order = postorder(f);

    // Sets only ever grow, so this terminates; a fixed sweep cap would hand
    // DCE an under-approximation (a live register called dead) on CFGs whose
    // layout runs against the control flow.
    let mut first = true;
    let mut changed = true;
    while changed {
        changed = false;
        for &b in &order {
            let mut grew = first;
            for &s in &succ[b] {
                let add: Vec<u32> = ins[s].iter().copied().filter(|r| !outs[b].contains(r)).collect();
                if !add.is_empty() {
                    grew = true;
                    outs[b].extend(add);
                }
            }
            if grew {
                let mut add: Vec<u32> = gen[b].iter().copied().filter(|r| !ins[b].contains(r)).collect();
                add.extend(outs[b].iter().copied().filter(|r| !kill[b].contains(r) && !ins[b].contains(r)));
                if !add.is_empty() {
                    changed = true;
                    ins[b].extend(add);
                }
            }
        }
        first = false;
    }
    let mut live_in: HashMap<u32, LiveSet> = HashMap::with_capacity(n);
    let mut live_out: HashMap<u32, LiveSet> = HashMap::with_capacity(n);
    for (i, bb) in f.blocks.iter().enumerate().rev() {
        live_in.insert(bb.id.0, std::mem::take(&mut ins[i]));
        live_out.insert(bb.id.0, std::mem::take(&mut outs[i]));
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

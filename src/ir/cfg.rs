//! Control-flow graph export (Graphviz DOT).

use super::{IrModule, Terminator};

pub fn to_dot(module: &IrModule) -> String {
    let mut s = String::from("digraph aether {\n  rankdir=TB;\n");
    for f in &module.functions {
        s.push_str(&format!("  subgraph cluster_{} {{\n", sanitize(&f.name)));
        s.push_str(&format!("    label=\"{}\";\n", f.name));
        for bb in &f.blocks {
            let label = format!("{}\\n{} insts", bb.id, bb.insts.len());
            s.push_str(&format!(
                "    {n} [shape=box,label=\"{label}\"];\n",
                n = node(&f.name, bb.id.0)
            ));
        }
        for bb in &f.blocks {
            let from = node(&f.name, bb.id.0);
            match &bb.term {
                Terminator::Jump { target } => {
                    s.push_str(&format!("    {from} -> {};\n", node(&f.name, target.0)));
                }
                Terminator::Branch {
                    then_bb, else_bb, ..
                } => {
                    s.push_str(&format!(
                        "    {from} -> {} [label=\"T\"];\n",
                        node(&f.name, then_bb.0)
                    ));
                    s.push_str(&format!(
                        "    {from} -> {} [label=\"F\"];\n",
                        node(&f.name, else_bb.0)
                    ));
                }
                Terminator::Return { .. } => {}
                Terminator::Unreachable => {}
            }
        }
        s.push_str("  }\n");
    }
    s.push_str("}\n");
    s
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn node(func: &str, id: u32) -> String {
    format!("{}_{id}", sanitize(func))
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
    fn dot_mentions_main() {
        let src = "fn main() -> i32 { if true { return 1; } return 0; }";
        let (toks, _) = tokenize(FileId(0), src);
        let (prog, _) = parse(toks);
        let (hir, _) = analyze(&prog);
        let ir = emit_ir(&hir.unwrap());
        let dot = to_dot(&ir);
        assert!(dot.contains("digraph"));
        assert!(dot.contains("main"));
    }
}

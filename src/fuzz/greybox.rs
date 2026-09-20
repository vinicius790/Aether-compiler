//! In-tree greybox loop: coverage-guided mutation of Aether IR.
//!
//! Why not cargo-fuzz / AFL++
//! --------------------------
//! Those need nightly + sanitizers + an `Arbitrary` derive that our 1.75
//! CI image does not host. The *algorithm* still fits in a page:
//!
//! 1. Seed a corpus from `gen_ir`.
//! 2. Pick a seed with energy ∝ (1 + unique edges it discovered).
//! 3. Mutate it (`mutate_ir`).
//! 4. Assemble `-O0`, run the VM with edge coverage.
//! 5. If the run paints a new edge, keep the mutant and raise its energy.
//! 6. Also check `-O0` vs `-O2` (the same oracle as `--kind mir`).
//!
//! Coverage is `(prev_site, site)` hashed in the VM (`execute_with_coverage`),
//! not SanitizerCoverage. It is enough to show the schedule working and to
//! prefer mutants that reach a decoy join, a helper call, or a loop body.

use super::mir::{check_ir, eval_ir, gen_ir, mutate_ir};
use super::rng::FuzzRng;
use crate::backend::assemble;
use crate::ir::{dump_ir, IrModule};
use crate::vm::execute_with_coverage;
use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};

#[derive(Debug, Clone)]
pub struct GreyboxReport {
    pub iters: u32,
    pub corpus: usize,
    pub edges: usize,
    pub new_edge_finds: u32,
    pub failures: u32,
    pub last_failure: Option<String>,
}

impl GreyboxReport {
    pub fn summary(&self) -> String {
        format!(
            "greybox: iters={} corpus={} edges={} finds={} fail={}\n",
            self.iters, self.corpus, self.edges, self.new_edge_finds, self.failures
        )
    }
}

struct Seed {
    module: IrModule,
    energy: u32,
}

pub fn run_greybox(iters: u32, seed: u64) -> GreyboxReport {
    let mut rng = FuzzRng::new(seed ^ 0x6BE0_9B);
    let mut corpus: Vec<Seed> = (0..4)
        .map(|_| Seed {
            module: gen_ir(&mut rng),
            energy: 2,
        })
        .collect();
    let mut global: HashSet<u64> = HashSet::new();
    let mut finds = 0u32;
    let mut failures = 0u32;
    let mut last_failure = None;

    for s in &corpus {
        if let Ok(e) = coverage_of(&s.module) {
            global.extend(e);
        }
    }

    for _ in 0..iters {
        let idx = pick(&mut rng, &corpus);
        let mutant = if rng.bool() {
            mutate_ir(&mut rng, &corpus[idx].module)
        } else {
            gen_ir(&mut rng)
        };
        if check_ir(&mutant).is_err() {
            continue;
        }

        match catch_unwind(AssertUnwindSafe(|| eval_pair(&mutant))) {
            Ok(Err(e)) => {
                failures += 1;
                if last_failure.is_none() {
                    last_failure = Some(format!("{e}\n{}", dump_ir(&mutant)));
                }
            }
            Err(_) => {
                failures += 1;
                if last_failure.is_none() {
                    last_failure = Some(format!("panic\n{}", dump_ir(&mutant)));
                }
            }
            Ok(Ok(())) => {}
        }

        if let Ok(edges) = coverage_of(&mutant) {
            let novel = edges.iter().filter(|e| !global.contains(e)).count();
            if novel > 0 {
                global.extend(edges);
                finds += 1;
                corpus.push(Seed {
                    module: mutant,
                    energy: 2 + novel as u32,
                });
                corpus[idx].energy = corpus[idx].energy.saturating_add(1);
            }
        }
    }

    GreyboxReport {
        iters,
        corpus: corpus.len(),
        edges: global.len(),
        new_edge_finds: finds,
        failures,
        last_failure,
    }
}

fn pick(rng: &mut FuzzRng, corpus: &[Seed]) -> usize {
    let total: u32 = corpus.iter().map(|s| s.energy.max(1)).sum();
    let mut ticket = rng.int(0, total.saturating_sub(1) as i32) as u32;
    for (i, s) in corpus.iter().enumerate() {
        if ticket < s.energy.max(1) {
            return i;
        }
        ticket -= s.energy.max(1);
    }
    0
}

fn coverage_of(module: &IrModule) -> Result<HashSet<u64>, String> {
    let bc = assemble(module);
    execute_with_coverage(&bc)
        .map(|(_, _, _, edges)| edges)
        .map_err(|e| e.to_string())
}

fn eval_pair(module: &IrModule) -> Result<(), String> {
    let a = eval_ir(module.clone(), 0)?;
    let b = eval_ir(module.clone(), 2)?;
    if a.0 != b.0 {
        return Err(format!("value O0={:?} O2={:?}", a.0, b.0));
    }
    if a.1 != b.1 {
        return Err(format!("stdout O0={:?} O2={:?}", a.1, b.1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greybox_grows_a_corpus() {
        let r = run_greybox(24, 7);
        assert!(r.corpus >= 4);
        assert!(r.edges > 0);
        assert_eq!(r.failures, 0, "{}", r.last_failure.unwrap_or_default());
    }
}

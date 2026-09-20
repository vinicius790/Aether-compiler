# Security policy

## Supported versions

| Version | Supported |
|---------|-----------|
| 0.2.x   | yes       |
| 0.1.x   | no        |

Aether is a research compiler. The VM is the only supported execution
engine. The LLVM textual backend is not a security boundary.

## What to report

- Panics or out-of-bounds access in the compiler or VM on untrusted source
- Optimizer miscompilation that changes observable results (`-O0` ≠ `-O2`)
- Unbounded loops introduced by a transformation (not by the source)

Do **not** report: missing language features, slow compile times, or
LLVM text that `llc` rejects.

## How to report

Open a GitHub issue with the label `security` **or**, if the issue is
exploitable on a host that runs untrusted `.ae` files, email the
maintainers listed in AUTHORS and wait 90 days before public disclosure.

Include: Aether version, seed (`aether fuzz --seed`), reduced source,
and `aether dump-ir --unopt` / `-O2` dumps.

## Fuzzer findings

A failing `aether fuzz` run with a printed seed is a valid report.
Replay with the same `--kind` and `--seed`.

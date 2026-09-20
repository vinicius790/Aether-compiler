#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
kind="${1:-all}"
iters="${2:-500}"
seed="${3:-1}"
cargo run --quiet -- fuzz --kind "$kind" --iters "$iters" --seed "$seed"

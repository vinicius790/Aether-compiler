#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
echo "=== Aether source line counts ==="
find src docs examples benchmarks tests -type f \
  \( -name '*.rs' -o -name '*.md' -o -name '*.ae' \) \
  | sort | xargs wc -l

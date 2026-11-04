#!/bin/sh
# Main Phase 2 vs Phase 3 comparison (3 repetitions, median). Usage: run_main.sh OUT.json
cd "$(dirname "$0")/.."
P2=bin/kiln-phase2; P3=bin/kiln-phase3
python3 tools/bench.py --data data --work work --out "$1" --reps 3 \
  --variants "p2-zstd3|$P2|-c zstd:3" "p3-balanced|$P3|-p balanced" "p3-adaptive|$P3|-p adaptive" \
             "p2-zstd1|$P2|-c zstd:1" "p3-fastest|$P3|-p fastest" \
             "p2-store|$P2|-c store" "p3-store|$P3|-p store" \
  --datasets d1_pytree d3_mixed d4_large.bin d5_random

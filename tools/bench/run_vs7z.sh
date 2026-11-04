#!/bin/sh
# Kiln vs 7-Zip vs ZIP. Runs three batches; see docs/PHASE3.md for what each one means.
cd "$(dirname "$0")/.."
mkdir -p work/v /dev/shm/vs7z results
python3 tools/vs7z.py --data data --work work/v --out results/vs7z-a.json d3_mixed d5_random d1_pytree
python3 tools/vs7z.py --data data --work work/v --out results/vs7z-b.json --skip-max d4_large.bin
python3 tools/vs7z.py --data data --work /dev/shm/vs7z --out results/vs7z-c-tmpfs.json --skip-max d2_tiny
echo ALLDONE

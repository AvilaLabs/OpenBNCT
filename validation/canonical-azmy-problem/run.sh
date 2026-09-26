#!/usr/bin/env bash
# Azmy's (1988) weighted-DD quadrant problem: one solve on the
# full-domain square, scored against the published quadrant means.
set -euo pipefail
cd "$(dirname "$0")"

BIN="${OPENBNCT:-openbnct}"

[ -f case.json ] || python3 generate.py

# c = sigma_s/sigma_t = 0.5 (source) and 0.05 (absorber) — benign
# source iteration; ordinary budgets converge.
"$BIN" sn solve \
    --case case.json \
    --data multigroup-data.json \
    --assignment assignment.json \
    --periodic z \
    --order 8 --convergence 2e-5 --inner-convergence 2e-5 \
    --max-inner 2000 --max-outer 8 \
    --output flux.json

python3 compare.py

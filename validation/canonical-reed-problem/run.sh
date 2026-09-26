#!/usr/bin/env bash
# Reed's problem: two volumetric-source solves on the mirror-domain
# slab, superposed and scored against the Warsa (2002) eigenfunction
# reference by compare.py.
#
#   OPENBNCT=target/debug/openbnct ./run.sh   (from repo root paths)
set -euo pipefail
cd "$(dirname "$0")"

BIN="${OPENBNCT:-openbnct}"

[ -f case-src1.json ] || python3 generate.py

# Solve A — strong source slab |x|<2 (q=50/cm3).
# Absorber-dominated region: converges quickly.
"$BIN" sn solve \
    --case case-src1.json \
    --data multigroup-data.json \
    --assignment assignment.json \
    --periodic y,z \
    --order 8 --convergence 2e-5 --inner-convergence 2e-5 \
    --max-inner 1500 --max-outer 8 \
    --output flux-src1.json

# Solve B — one copy of the weak scattering source, x in [5,6] cm
# (q=1/cm3). The c=0.9 regions are iteration-slow (rho~0.99) and the
# positivity clamps in the near-void cells floor the relative-change
# iterate near ~2e-5 — tolerances are set just above that floor; the
# physical flux is stable far below the comparison thresholds. Its
# mirror copy is supplied in compare.py.
"$BIN" sn solve \
    --case case-src2.json \
    --data multigroup-data.json \
    --assignment assignment.json \
    --periodic y,z \
    --order 8 --convergence 2e-5 --inner-convergence 2e-5 \
    --max-inner 1500 --max-outer 8 \
    --output flux-src2.json

python3 compare.py

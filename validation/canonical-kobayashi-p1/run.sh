#!/usr/bin/env bash
# Kobayashi problem 1, both published cases on the mirrored
# nested-cubes domain (2 cm cells, S8):
#   case i  - pure absorber, scored against the cell-averaged
#             analytic ray integral. Deep-field probes carry the
#             classic ray effect: the script reports them but only
#             grades the near/mid field, where DD is legitimate.
#   case ii - 50% scattering, scored against the GMVP MC reference.
set -euo pipefail
cd "$(dirname "$0")"

BIN="${OPENBNCT:-openbnct}"

[ -f case-i.json ] || python3 generate.py
[ -f reference/kobayashi-p1-cellmean.csv ] || python3 reference.py

"$BIN" sn solve \
    --case case-i.json \
    --data multigroup-data-i.json \
    --assignment assignment.json \
    --order 8 --convergence 1e-8 --inner-convergence 1e-8 \
    --max-inner 400 --max-outer 8 \
    --output flux.json

python3 compare.py

"$BIN" sn solve \
    --case case-ii.json \
    --data multigroup-data-ii.json \
    --assignment assignment.json \
    --order 8 --convergence 2e-5 --inner-convergence 2e-5 \
    --max-inner 2000 --max-outer 8 \
    --output flux-ii.json

python3 compare_ii.py

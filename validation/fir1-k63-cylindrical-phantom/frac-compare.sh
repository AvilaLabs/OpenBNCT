#!/bin/bash
# Compare the boundary-fraction assignment (v2) against the binary
# center-inside assignment (v1) on the cylindrical phantom: QA chain,
# axial flux profile, dose components, and measurement agreement.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
BIN=../../target/release/openbnct

"$BIN" beam qa --beam ../../beams/fir1-k63.json \
  --dose "$ROOT/dose-28g-tsl-p1-frac-1e.json" \
  --flux "$ROOT/multigroup-flux-28g-tsl-p1-frac-1e.json" \
  --transverse-depth-cm 2.0,6.0 \
  --tumor-weights B=1995,N=3.2,H=3.2,P=1.0 \
  --normal-weights B=150,N=3.2,H=3.2,P=1.0 \
  --report-id openbnct.beam-quality.fir1-k63-cylindrical.28g-tsl-p1-frac-1e.v1 \
  --output "$ROOT/beam-quality-cylindrical-28g-tsl-p1-frac-1e.json"

"$BIN" measurement compare \
  --record ../../measurements/fir1-k63-cylindrical-phantom-depth.json \
  --against "$ROOT/beam-quality-cylindrical-28g-tsl-p1-frac-1e.json" \
  --report-id openbnct.measurement-comparison.fir1-k63-cylindrical.28g-tsl-p1-frac-1e.v1 \
  --output "$ROOT/measurement-comparison-cylindrical-28g-tsl-p1-frac-1e.json"

"$BIN" bio apply --model models/cbe-protocol.json \
  --physical-bundle "$ROOT/dose-28g-tsl-p1-frac-1e.json" \
  --region-mask tumor=region-tumor.json \
  --output "$ROOT/bio-cbe-28g-tsl-p1-frac-1e.json"

"$BIN" bio apply --model models/mkm-literature-constants.json \
  --physical-bundle "$ROOT/dose-28g-tsl-p1-frac-1e.json" \
  --output "$ROOT/bio-mkm-28g-tsl-p1-frac-1e.json"

python3 - <<'PY'
import json
f1 = json.load(open('multigroup-flux-28g-tsl-p1-1e.json'))
f2 = json.load(open('multigroup-flux-28g-tsl-p1-frac-1e.json'))
d1 = json.load(open('dose-28g-tsl-p1-1e.json'))
d2 = json.load(open('dose-28g-tsl-p1-frac-1e.json'))
nx, ny, nz = 24, 24, 24
cx = nx // 2
print('z(mm) | binary thermal | frac thermal | ratio')
for k in range(0, nz, 2):
    cell = cx + nx * cx + nx * ny * k
    b, f = f1['flux'][cell][27], f2['flux'][cell][27]
    print(f'{k*10:5.0f}  {b:12.5e}  {f:12.5e}  {f/b if b else float("nan"):7.4f}')
for c1, c2 in zip(d1['components'], d2['components']):
    s1, s2 = sum(c1['values']), sum(c2['values'])
    print(f'dose {c1["component"]:>8}: binary {s1:.4e}  frac {s2:.4e}  ratio {s2/s1:.4f}')
PY

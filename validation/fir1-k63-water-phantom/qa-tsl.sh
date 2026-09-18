#!/bin/bash
# k63 water-phantom QA chain for the TSL+P1 solve:
# beam qa -> measurement compare -> component dose check vs OpenMC.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
BIN=../../target/release/openbnct

"$BIN" beam qa --beam ../../beams/fir1-k63.json \
  --dose "$ROOT/dose-28g-tsl-p1-1e.json" \
  --flux "$ROOT/multigroup-flux-28g-tsl-p1-1e.json" \
  --transverse-depth-cm 2.0,6.0 \
  --tumor-weights B=1995,N=3.2,H=3.2,P=1.0 \
  --normal-weights B=150,N=3.2,H=3.2,P=1.0 \
  --report-id openbnct.beam-quality.fir1-k63-water-phantom.28g-tsl-p1-1e.v1 \
  --output "$ROOT/beam-quality-water-28g-tsl-p1-1e.json"

"$BIN" measurement compare \
  --record ../../measurements/fir1-k63-water-phantom.json \
  --against "$ROOT/beam-quality-water-28g-tsl-p1-1e.json" \
  --report-id openbnct.measurement-comparison.fir1-k63-water-phantom.28g-tsl-p1-1e.v1 \
  --output "$ROOT/measurement-comparison-water-28g-tsl-p1-1e.json"

# Component-by-component dose check against the OpenMC tallies on the
# identical 26x26x94 grid (same geometry, same beam, same components).
python3 - <<'PY'
import json
EV = '/home/connoravila/Documents/Avila-Labs/.nctforge-data/nctforge/fir1-k63-water-phantom/openmc-evidence/normalized-dose.json'
det = json.load(open('dose-28g-tsl-p1-1e.json'))
mc = json.load(open(EV))
print('component | deterministic | OpenMC | ratio')
for mc_comp in mc['components']:
    name = mc_comp['component']
    dv = [c for c in det['components'] if c['component'] == name]
    if not dv:
        print(f'{name:>9} | (absent) | {sum(mc_comp["values"]):.4e}')
        continue
    d_sum = sum(dv[0]['values']); m_sum = sum(mc_comp['values'])
    print(f'{name:>9} | {d_sum:.4e} | {m_sum:.4e} | {d_sum/m_sum:.3f}')
PY

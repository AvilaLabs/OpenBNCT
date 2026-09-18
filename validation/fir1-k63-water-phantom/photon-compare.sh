#!/bin/bash
# Coupled n->gamma check: deterministic transported-photon dose vs the
# OpenMC photon-heating tally on the same 26x26x94 k63 water phantom.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"

openbnct sn photon-solve \
  --case "$ROOT/case.json" \
  --photon-data "$ROOT/multigroup-photon-data-16g.json" \
  --neutron-flux "$ROOT/multigroup-flux-28g-tsl-p1-1e.json" \
  --assignment "$ROOT/assignment.json" \
  --order 8 \
  --dose "$ROOT/dose-photon-16g.json" \
  --output "$ROOT/multigroup-photon-flux-16g.json"

python3 - <<'PY'
import json
EV = '/home/connoravila/Documents/Avila-Labs/.nctforge-data/nctforge/fir1-k63-water-phantom/openmc-evidence/normalized-dose.json'
tp = json.load(open('dose-photon-16g.json'))
mc = json.load(open(EV))
tpv = [c for c in tp['components'] if c['component'] == 'photon'][0]['values']
mcv = [c for c in mc['components'] if c['component'] == 'photon'][0]['values']
assert len(tpv) == len(mcv) == 63544, (len(tpv), len(mcv))
nx, ny, nz = 26, 26, 94
cx = nx // 2
print('z(cm) | transported | OpenMC | ratio')
for k in range(0, nz, 6):
    cell = cx + nx * cx + nx * ny * k
    t, m = tpv[cell], mcv[cell]
    print(f'{k * 0.5:5.1f}  {t:12.5e}  {m:12.5e}  {t / m if m else float("nan"):8.3f}')
tt, tm = sum(tpv), sum(mcv)
print('integral transported:', f'{tt:.4e}', ' OpenMC:', f'{tm:.4e}', ' ratio:', f'{tt / tm:.3f}')
PY

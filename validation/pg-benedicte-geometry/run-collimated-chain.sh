#!/usr/bin/env bash
# BeNEdiCTE-style two-vial reconstruction chain — pinhole-collimated
# variant. Each detector's adjoint source is restricted to the cone
# its aperture subtends, so each response column is a narrow ray
# bundle through the phantom — the selectivity the uncollimated
# chain lacked. Run inside the enforced cgroup per AGENTS.md.
set -euo pipefail
D="$(cd "$(dirname "$0")" && pwd)"
P="$(dirname "$D")/fir1-k63-cylindrical-phantom"
BIN="${OPENBNCT:-openbnct}"
OUT="$D/collimated"
mkdir -p "$OUT"
DETS=(1,11,12 22,11,12 11,1,12 11,22,12 4,4,12 19,4,12 4,19,12 19,19,12)
# Aperture centers: ~50 mm from each detector along the S8 ordinate
# nearest the detector→vial-region (0,0,125) axis, so each cone
# (r=15 mm ≈ 17°) captures that ordinate — one pencil line per
# detector through the imaged volume, the discrete-ordinates analog
# of a pinhole camera.
APS=(-52.4,10.9,119.1 62.4,10.9,119.1 10.9,-52.4,119.1 10.9,62.4,119.1 -41.1,-30.7,119.1 40.7,-41.1,119.1 -41.1,40.7,119.1 51.1,40.7,119.1)
i=0
for det in "${DETS[@]}"; do
  "$BIN" pg response \
    --case "$P/case.json" \
    --photon-data "$P/multigroup-photon-data-16g.json" \
    --detector "$det" --aperture="${APS[$i]}" --aperture-radius-mm 15 \
    --order 8 --convergence 1e-5 --max-outer 16 \
    --id "pg-response-coll-det$i" --output "$OUT/response-det$i.json"
  "$BIN" pg counts \
    --emission "$D/emission-two-vials.json" \
    --response "$OUT/response-det$i.json" \
    --density-kg-per-m3 1000 --efficiency 0.6 \
    --id "pg-counts-coll-det$i" --output "$OUT/counts-det$i.json"
  i=$((i+1))
done
"$BIN" pg observe \
  $(for j in $(seq 0 7); do printf -- "--counts %s " "$OUT/counts-det$j.json"; done) \
  --id pg-observation-coll --output "$OUT/observation.json"
"$BIN" pg reconstruct \
  --observation "$OUT/observation.json" \
  $(for j in $(seq 0 7); do printf -- "--response %s " "$OUT/response-det$j.json"; done) \
  --density-kg-per-m3 1000 --lambda 1e-6 \
  --id pg-reconstruction-coll --output "$OUT/reconstruction.json"

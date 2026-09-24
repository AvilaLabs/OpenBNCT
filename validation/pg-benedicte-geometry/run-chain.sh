#!/usr/bin/env bash
# BeNEdiCTE-style two-vial reconstruction chain — the transport steps.
# Run inside the enforced cgroup per AGENTS.md.
set -euo pipefail
D="$(cd "$(dirname "$0")" && pwd)"
P="$(dirname "$D")/fir1-k63-cylindrical-phantom"
BIN="${OPENBNCT:-openbnct}"
DETS=(1,11,12 22,11,12 11,1,12 11,22,12 4,4,12 19,4,12 4,19,12 19,19,12)
i=0
for det in "${DETS[@]}"; do
  "$BIN" pg response \
    --case "$P/case.json" \
    --photon-data "$P/multigroup-photon-data-16g.json" \
    --detector "$det" --order 4 --convergence 1e-5 --max-outer 16 \
    --id "pg-response-det$i" --output "$D/response-det$i.json"
  "$BIN" pg counts \
    --emission "$D/emission-two-vials.json" \
    --response "$D/response-det$i.json" \
    --density-kg-per-m3 1000 --efficiency 0.6 \
    --id "pg-counts-det$i" --output "$D/counts-det$i.json"
  i=$((i+1))
done
"$BIN" pg observe \
  $(for j in $(seq 0 7); do printf -- "--counts %s " "$D/counts-det$j.json"; done) \
  --id pg-observation-two-vials --output "$D/observation.json"
"$BIN" pg reconstruct \
  --observation "$D/observation.json" \
  $(for j in $(seq 0 7); do printf -- "--response %s " "$D/response-det$j.json"; done) \
  --density-kg-per-m3 1000 --lambda 1e-4 \
  --id pg-reconstruction-two-vials --output "$D/reconstruction.json"

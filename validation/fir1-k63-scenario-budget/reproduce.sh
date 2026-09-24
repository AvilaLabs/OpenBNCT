#!/usr/bin/env bash
# Regenerates the fixture's committed outputs. Committed JSON is frozen
# evidence — run to verify, not to update.
set -euo pipefail
D="$(cd "$(dirname "$0")" && pwd)"
P="$(dirname "$D")/fir1-k63-cylindrical-phantom"
BIN="${OPENBNCT:-openbnct}"
"$BIN" plan optimize \
  --objective "$D/objective.json" \
  --dose "$P/dose-28g-tsl-p1-1e.json" \
  --mask "$D/mask-tumor.json" --mask "$D/mask-entrance.json" \
  --initial 1.0 \
  --id fir1-k63-budget-plan \
  --provenance-id fir1-k63-scenario-budget \
  --output "$D/result.json"
"$BIN" plan scenarios \
  --result "$D/result.json" \
  --objective "$D/objective.json" \
  --dose "$P/dose-28g-tsl-p1-1e.json" \
  --mask "$D/mask-tumor.json" --mask "$D/mask-entrance.json" \
  --scenario-set "$D/scenario-set.json" \
  --output "$D/scenario-report.json"

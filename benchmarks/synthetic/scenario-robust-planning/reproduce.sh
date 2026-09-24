#!/usr/bin/env bash
# Regenerates the fixture's committed outputs. Committed JSON is frozen
# evidence — run to verify, not to update.
set -euo pipefail
D="$(cd "$(dirname "$0")" && pwd)"
BIN="${OPENBNCT:-openbnct}"
"$BIN" plan optimize \
  --objective "$D/objective.json" \
  --dose "$D/field-h.json" --dose "$D/field-b.json" \
  --mask "$D/mask-all.json" \
  --initial 1.0 --initial 1.0 \
  --id scenario-robust-result \
  --provenance-id scenario-robust-planning-fixture \
  --scenario-set "$D/scenario-set.json" \
  --output "$D/result.json"
"$BIN" plan scenarios \
  --result "$D/result.json" \
  --objective "$D/objective.json" \
  --dose "$D/field-h.json" --dose "$D/field-b.json" \
  --mask "$D/mask-all.json" \
  --scenario-set "$D/scenario-set.json" \
  --output "$D/scenario-report.json"

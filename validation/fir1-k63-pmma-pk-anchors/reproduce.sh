#!/usr/bin/env bash
# Regenerates the fixture's committed outputs. Committed JSON is frozen
# evidence — run to verify, not to update.
set -euo pipefail
D="$(cd "$(dirname "$0")" && pwd)"
P="$(dirname "$D")/fir1-k63-pmma-phantom"
BIN="${OPENBNCT:-openbnct}"
"$BIN" pk tissue-scale \
  --blood-model "$D/blood-model.json" \
  --spec "$D/tissue-spec.json" \
  --id bpa-tissue-published \
  --output "$D/tissue-model.json"
"$BIN" pk schedule \
  --dose "$P/dose-28g-tsl-p1-1e.json" \
  --quantity component:boron \
  --source-strength 1e9 \
  --limit skin=mean:2.0e-2 \
  --mask "skin=$D/mask-skin.json" \
  --mask "gtv=$D/mask-gtv.json" \
  --pk-model "$D/tissue-model.json" \
  --window-s 0,1800,3600,5400,7200,10800 \
  --tumor-region gtv \
  --output "$D/schedule-report.json"

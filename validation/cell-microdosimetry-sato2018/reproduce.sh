#!/usr/bin/env bash
# Regenerates the fixture's committed outputs. Committed JSON is frozen
# evidence — run to verify, not to update.
set -euo pipefail
D="$(cd "$(dirname "$0")" && pwd)"
BIN="${OPENBNCT:-openbnct}"
for n in bpa bsh; do
  "$BIN" boron microdistribution evaluate \
    --model "$D/$n-microdistribution.json" \
    --id "$n-sato2018" --output "$D/$n-correction.json"
done
"$BIN" bio cell-microdosimetry --model "$D/bpa-microdistribution.json" \
  --mean-captures 20 --cells 20000 --seed 42 \
  --id bpa-sato2018 --output "$D/bpa-cell-microdosimetry.json"
"$BIN" bio cell-microdosimetry --model "$D/bsh-microdistribution.json" \
  --mean-captures 20 --cells 20000 --seed 42 \
  --id bsh-sato2018 --output "$D/bsh-cell-microdosimetry.json"
"$BIN" bio cell-microdosimetry --model "$D/bpa-microdistribution-hetero.json" \
  --mean-captures 20 --cells 20000 --seed 42 \
  --id bpa-sato2018-hetero --output "$D/bpa-hetero-cell-microdosimetry.json"
"$BIN" bio smk --cell-microdosimetry "$D/bpa-cell-microdosimetry.json" \
  --model "$D/bpa-microdistribution.json" \
  --alpha 0.0422 --beta 0.00822 --reference-alpha 0.0422 --reference-beta 0.00822 \
  --boron-dose-gy 5.0 --dose-levels-gy 1,2,4,8 \
  --id bpa-sato2018 --output "$D/bpa-smk-evaluation.json"
"$BIN" bio smk --cell-microdosimetry "$D/bsh-cell-microdosimetry.json" \
  --model "$D/bsh-microdistribution.json" \
  --alpha 0.0422 --beta 0.00822 --reference-alpha 0.0422 --reference-beta 0.00822 \
  --boron-dose-gy 5.0 --dose-levels-gy 1,2,4,8 \
  --id bsh-sato2018 --output "$D/bsh-smk-evaluation.json"
"$BIN" bio smk --cell-microdosimetry "$D/bpa-hetero-cell-microdosimetry.json" \
  --model "$D/bpa-microdistribution-hetero.json" \
  --alpha 0.0422 --beta 0.00822 --reference-alpha 0.0422 --reference-beta 0.00822 \
  --boron-dose-gy 5.0 --dose-levels-gy 1,2,4,8 \
  --id bpa-sato2018-hetero --output "$D/bpa-hetero-smk-evaluation.json"

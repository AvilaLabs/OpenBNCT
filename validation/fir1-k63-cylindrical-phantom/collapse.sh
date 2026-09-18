#!/bin/bash
# Collapse the cylindrical water phantom to 28-group data (v3):
# adds the P1 lab-cosine transfer moments alongside P0 + mu_bar.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
LIB=/home/connoravila/Documents/Avila-Labs/.nctforge-data/nctforge/openmc-endfb81-official-library/selected/endfb-viii.1-hdf5/neutron
PROFILE="$ROOT/../../benchmarks/synthetic/nf-bnct-001/transport/component-profile-local-kerma.json"
BOUNDS="1.69e7,1e7,6e6,4e6,2.5e6,1.5e6,1e6,7e5,5e5,3e5,2e5,1e5,6e4,3e4,1e4,6e3,3e3,1e3,5e2,1e2,30,10,3,1,0.5,0.1,0.025,0.01,1e-5"

openbnct sn collapse \
  --library "$LIB" \
  --material "$ROOT/../fir1-k63-water-phantom/material.json" \
  --material "$ROOT/material-void.json" \
  --boundaries "$BOUNDS" \
  --id openbnct.fir1-k63-cylindrical.multigroup-28g.v3 \
  --component-profile "$PROFILE" \
  --note "v3: emits scatter_p1_matrix_per_cm (iso-CM elastic P1 lab-cosine transfer moments) for the --p1 solve mode." \
  --output "$ROOT/multigroup-data-28g-v3.json"

#!/bin/bash
# Coupled photon transport for the cylindrical water phantom:
# photon-atomic collapse (16 photon groups) bound to the 28-group
# TSL neutron data, then a two-pass n->gamma photon solve driven by
# the converged TSL+P1 neutron flux.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
LIB=/home/connoravila/Documents/Avila-Labs/.nctforge-data/nctforge/openmc-endfb81-official-library/selected/endfb-viii.1-hdf5
PBOUNDS="1e7,5e6,3e6,2e6,1.5e6,1e6,7e5,5.5e5,5e5,4e5,3e5,2e5,1e5,5e4,2e4,1e4,1e3"

openbnct sn photon-collapse \
  --photon-library "$LIB/photon" \
  --neutron-library "$LIB/neutron" \
  --material "$ROOT/../fir1-k63-water-phantom/material.json" \
  --material "$ROOT/material-void.json" \
  --photon-boundaries "$PBOUNDS" \
  --neutron-data "$ROOT/multigroup-data-28g-tsl-v4.json" \
  --component-profile "$ROOT/models/component-profile-transported-photon.json" \
  --id openbnct.fir1-k63-cylindrical.photon-data-16g.v1 \
  --output "$ROOT/multigroup-photon-data-16g.json"

openbnct sn photon-solve \
  --case "$ROOT/case.json" \
  --photon-data "$ROOT/multigroup-photon-data-16g.json" \
  --neutron-flux "$ROOT/multigroup-flux-28g-tsl-p1-1e.json" \
  --assignment "$ROOT/assignment.json" \
  --order 4 \
  --dose "$ROOT/dose-photon-16g.json" \
  --output "$ROOT/multigroup-photon-flux-16g.json"

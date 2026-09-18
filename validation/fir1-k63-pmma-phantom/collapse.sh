#!/bin/bash
# Collapse the PMMA (Lucite) phantom to 28-group data:
# v1 is the committed free-gas artifact; the -tsl-v2 artifact adds the
# ENDF/B-VIII.1 H(Lucite) S(alpha,beta) bound-atom kernel on H1 below
# the tape's E_max (thermal upscatter included).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
LIB=/home/connoravila/Documents/Avila-Labs/.nctforge-data/nctforge/openmc-endfb81-official-library/selected/endfb-viii.1-hdf5/neutron
TSL=/home/connoravila/Documents/Avila-Labs/.nctforge-data/nctforge/endfb81-tsl-v1/evaluations
PROFILE="$ROOT/../../benchmarks/synthetic/nf-bnct-001/transport/component-profile-local-kerma.json"
BOUNDS="1.69e7,1e7,6e6,4e6,2.5e6,1.5e6,1e6,7e5,5e5,3e5,2e5,1e5,6e4,3e4,1e4,6e3,3e3,1e3,5e2,1e2,30,10,3,1,0.5,0.1,0.025,0.01,1e-5"

openbnct sn collapse \
  --library "$LIB" \
  --material "$ROOT/material.json" \
  --material "$ROOT/../fir1-k63-cylindrical-phantom/material-void.json" \
  --tsl "H1=$TSL/tsl_H(Lucite)_0039.dat" \
  --tsl-temperature 293.6 \
  --boundaries "$BOUNDS" \
  --id openbnct.fir1-k63-pmma-phantom.multigroup-28g-tsl.v2 \
  --component-profile "$PROFILE" \
  --note "v2: lwtr-Lucite S(a,b) bound-atom kernel on H1 below E_max=5 eV (sigma_b/natom*((A+1)/A)^2 = 82.14 b convention, NJOY/THERMR convention); free-gas residual beyond the beta domain." \
  --output "$ROOT/multigroup-data-28g-tsl-v2.json"

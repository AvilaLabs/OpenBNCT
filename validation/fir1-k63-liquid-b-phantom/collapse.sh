#!/bin/bash
# Collapse the Liquid B brain-substitute material to 28-group data.
# Same pipeline as the layered-head phantom: HDF5 for H/C/N/O/B10,
# NJOY PENDF (ENDF/B-VIII.1, 293.6 K) for Na/P/S/Cl/K minors.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
LIB=/home/connoravila/Documents/Avila-Labs/.nctforge-data/nctforge/openmc-endfb81-official-library/selected/endfb-viii.1-hdf5/neutron
PENDF=/home/connoravila/Documents/Avila-Labs/.nctforge-data/nctforge/endfb81-tissue-pendf-v1
PROFILE="$ROOT/../../benchmarks/synthetic/nf-bnct-001/transport/component-profile-local-kerma.json"
BOUNDS="1.69e7,1e7,6e6,4e6,2.5e6,1.5e6,1e6,7e5,5e5,3e5,2e5,1e5,6e4,3e4,1e4,6e3,3e3,1e3,5e2,1e2,30,10,3,1,0.5,0.1,0.025,0.01,1e-5"

ENDF_ARGS=()
for spec in \
  "Na23 n-011_Na_023" "P31 n-015_P_031" \
  "S32 n-016_S_032" "S33 n-016_S_033" "S34 n-016_S_034" \
  "Cl35 n-017_Cl_035" "Cl37 n-017_Cl_037" \
  "K39 n-019_K_039" "K40 n-019_K_040" "K41 n-019_K_041"; do
  set -- $spec
  ENDF_ARGS+=(--endf "$1=$PENDF/$2/tape22")
done

TSL=/home/connoravila/Documents/Avila-Labs/.nctforge-data/nctforge/endfb81-tsl-v1/evaluations

openbnct sn collapse \
  --library "$LIB" \
  "${ENDF_ARGS[@]}" \
  --material "$ROOT/material-liquid-b.json" \
  --material "$ROOT/material-void.json" \
  --boundaries "$BOUNDS" \
  --id openbnct.fir1-k63-liquid-b-phantom.multigroup-28g.v1 \
  --component-profile "$PROFILE" \
  --note "Liquid B brain-tissue-substitute surrogate (ICRU brain elemental basis, rho 1.04); H/C/N/O/B10 from 294K HDF5, minors from NJOY 293.6K PENDF (ENDF/B-VIII.1). Free-gas treatment — H(H2O) TSL follow-on." \
  --output "$ROOT/multigroup-data-28g.json"

openbnct sn collapse \
  --library "$LIB" \
  "${ENDF_ARGS[@]}" \
  --material "$ROOT/material-liquid-b.json" \
  --material "$ROOT/material-void.json" \
  --tsl "H1=$TSL/tsl_H(H2O)_0001.dat" \
  --tsl-temperature 293.6 \
  --boundaries "$BOUNDS" \
  --id openbnct.fir1-k63-liquid-b-phantom.multigroup-28g-tsl.v2 \
  --component-profile "$PROFILE" \
  --note "v2: v1 + lwtr S(a,b) bound-atom kernel on H1 below E_max=10 eV — standard lwtr treatment for tissue hydrogen (Liquid B is a water-rich brain surrogate); sigma_b/natom*((A+1)/A)^2 = 81.81 b, free-gas residual beyond the beta domain." \
  --output "$ROOT/multigroup-data-28g-tsl-v2.json"

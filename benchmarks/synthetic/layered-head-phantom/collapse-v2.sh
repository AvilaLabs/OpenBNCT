#!/bin/bash
# Collapse the layered-head phantom materials to 28-group data (v2: the
# corrected hydrogen recoil kerma; v1 = collapse.sh, kept as the record).
# Requires: NJOY-processed PENDF tapes for the tissue nuclides
# (process-tissue-pendf.sh in the data store) and the processed
# ENDF/B-VIII.1 OpenMC-HDF5 library for H/C/N/O/B10.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
LIB=/home/connoravila/Documents/Avila-Labs/.nctforge-data/nctforge/openmc-endfb81-official-library/selected/endfb-viii.1-hdf5/neutron
PENDF=/home/connoravila/Documents/Avila-Labs/.nctforge-data/nctforge/endfb81-tissue-pendf-v1
PROFILE="$ROOT/../nf-bnct-001/transport/component-profile-local-kerma.json"
BOUNDS="1.69e7,1e7,6e6,4e6,2.5e6,1.5e6,1e6,7e5,5e5,3e5,2e5,1e5,6e4,3e4,1e4,6e3,3e3,1e3,5e2,1e2,30,10,3,1,0.5,0.1,0.025,0.01,1e-5"

ENDF_ARGS=()
for spec in \
  "Na23 n-011_Na_023" "Mg24 n-012_Mg_024" "Mg25 n-012_Mg_025" "Mg26 n-012_Mg_026" \
  "P31 n-015_P_031" "S32 n-016_S_032" "S33 n-016_S_033" "S34 n-016_S_034" \
  "Cl35 n-017_Cl_035" "Cl37 n-017_Cl_037" \
  "K39 n-019_K_039" "K40 n-019_K_040" "K41 n-019_K_041" \
  "Ca40 n-020_Ca_040" "Ca42 n-020_Ca_042" "Ca43 n-020_Ca_043" \
  "Ca44 n-020_Ca_044" "Ca46 n-020_Ca_046" "Ca48 n-020_Ca_048"; do
  set -- $spec
  ENDF_ARGS+=(--endf "$1=$PENDF/$2/tape22")
done

openbnct sn collapse \
  --library "$LIB" \
  "${ENDF_ARGS[@]}" \
  --material "$ROOT/materials/skin.json" \
  --material "$ROOT/materials/skull.json" \
  --material "$ROOT/materials/brain.json" \
  --material "$ROOT/materials/void.json" \
  --boundaries "$BOUNDS" \
  --id openbnct.layered-head-phantom.multigroup-28g.v2 \
  --component-profile "$PROFILE" \
  --note "ICRU-44 tissues; H/C/N/O/B10 from 294K HDF5, tissue nuclides from NJOY 293.6K PENDF (ENDF/B-VIII.1). v2: hydrogen recoil kerma converted eV to MeV (supersedes v1, whose hydrogen responses were 1e6 too large)" \
  --output "$ROOT/multigroup-data-28g-v2.json"

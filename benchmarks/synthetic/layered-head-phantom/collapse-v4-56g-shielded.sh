#!/bin/bash
# Collapse the layered-head phantom materials to 56-group
# self-shielded data (v4: lethargy-bisected 28g boundaries + the v3
# Bondarenko dilution — the resonance-region resolution and the
# shielding correction in one library).
# Requires: NJOY-processed PENDF tapes for the tissue nuclides and the
# processed ENDF/B-VIII.1 OpenMC-HDF5 library for H/C/N/O/B10.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
LIB=/home/connoravila/Documents/Avila-Labs/.nctforge-data/nctforge/openmc-endfb81-official-library/selected/endfb-viii.1-hdf5/neutron
PENDF=/home/connoravila/Documents/Avila-Labs/.nctforge-data/nctforge/endfb81-tissue-pendf-v1
PROFILE="$ROOT/../nf-bnct-001/transport/component-profile-local-kerma.json"
# 28g boundaries bisected in lethargy — every v2/v3 group edge is also
# a v4 edge, so shielded-vs-unshielded and coarse-vs-fine factors
# separate cleanly.
BOUNDS="1.69e7,1.3e7,1e7,7.74597e6,6e6,4.89898e6,4e6,3.16228e6,2.5e6,1.93649e6,1.5e6,1.22474e6,1e6,836660,7e5,591608,5e5,387298,3e5,244949,2e5,141421,1e5,77459.7,6e4,42426.4,3e4,17320.5,1e4,7745.97,6e3,4242.64,3e3,1732.05,1e3,707.107,5e2,223.607,1e2,54.7723,30,17.3205,10,5.47723,3,1.73205,1,0.707107,0.5,0.223607,0.1,0.05,0.025,0.0158114,0.01,0.000316228,1e-5"
OPENBNCT="${OPENBNCT:-$ROOT/../../../target/debug/openbnct}"

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

"$OPENBNCT" sn collapse \
  --library "$LIB" \
  "${ENDF_ARGS[@]}" \
  --material "$ROOT/materials/skin.json" \
  --material "$ROOT/materials/skull.json" \
  --material "$ROOT/materials/brain.json" \
  --material "$ROOT/materials/void.json" \
  --boundaries "$BOUNDS" \
  --self-shielding \
  --id openbnct.layered-head-phantom.multigroup-56g.v1-shielded \
  --component-profile "$PROFILE" \
  --note "ICRU-44 tissues; H/C/N/O/B10 from 294K HDF5, tissue nuclides from NJOY 293.6K PENDF (ENDF/B-VIII.1). 56g lethargy-bisected structure with Bondarenko self-shielding — the condensation-resolution and dilution-corrected reference" \
  --output "$ROOT/multigroup-data-56g-shielded.json"

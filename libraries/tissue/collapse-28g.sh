#!/bin/bash
# Collapse the standard tissue library to 28-group neutron data with the
# H-in-H2O thermal-scattering law (ENDF/B-VIII.1 tsl_H(H2O)_0001) on every
# hydrogen. The earlier free-gas-only collapse (multigroup-data-28g.json,
# id ...multigroup-28g-tsl.v1) is kept for provenance; regenerate it by dropping
# the --tsl argument and using its id/output.
#
# Same recipe as benchmarks/synthetic/layered-head-phantom/collapse-v2.sh
# (28-group grid, Maxwellian+1/E default weighting, free-gas elastic kernel,
# no self-shielding, local-kerma component profile): H/C/N/O/Fe56/B10 from the
# 294 K ENDF/B-VIII.1 OpenMC-HDF5 library; Na/Mg/P/S/Cl/K/Ca from NJOY
# 293.6 K PENDF tapes (process-tissue-pendf.sh in the data store).
# The emitted data carry boron_unit_response_gy_cm2_per_ug_g (sn collapse,
# e269933) and are bound to the local-kerma component profile so
# `sn solve --dose` works.
#
# Requires the external nuclear-data store (not in the repository); override
# with NCTFORGE_DATA if it lives elsewhere. OPENBNCT selects the binary.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
DATA="${NCTFORGE_DATA:-/home/connoravila/Documents/Avila-Labs/.nctforge-data/nctforge}"
LIB="$DATA/openmc-endfb81-official-library/selected/endfb-viii.1-hdf5/neutron"
PENDF="$DATA/endfb81-tissue-pendf-v1"
TSL_H="$DATA/endfb81-tsl-v1/evaluations/tsl_H(H2O)_0001.dat"
PROFILE="$ROOT/../../benchmarks/synthetic/nf-bnct-001/transport/component-profile-local-kerma.json"
BOUNDS="1.69e7,1e7,6e6,4e6,2.5e6,1.5e6,1e6,7e5,5e5,3e5,2e5,1e5,6e4,3e4,1e4,6e3,3e3,1e3,5e2,1e2,30,10,3,1,0.5,0.1,0.025,0.01,1e-5"
OPENBNCT="${OPENBNCT:-openbnct}"
OUT="${OUT:-$ROOT/multigroup-data-28g-tsl.json}"
MATERIALS="${MATERIALS:-air-dry water-liquid adipose-icrp soft-tissue-icrp muscle-skeletal-icrp brain-icrp skin-icrp blood-icrp lung-icrp lung-inflated-declared cortical-bone-icrp}"

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

MAT_ARGS=()
for m in $MATERIALS; do MAT_ARGS+=(--material "$ROOT/materials/$m.json"); done

"$OPENBNCT" sn collapse \
  --library "$LIB" \
  "${ENDF_ARGS[@]}" \
  --tsl "H1=$TSL_H" \
  "${MAT_ARGS[@]}" \
  --boundaries "$BOUNDS" \
  --id openbnct.tissue-library.multigroup-28g-tsl.v1 \
  --component-profile "$PROFILE" \
  --note "Standard tissue library (PNNL-15870 Rev. 1 compositions); H/C/N/O/Fe56/B10 from 294K HDF5, Na/Mg/P/S/Cl/K/Ca from NJOY 293.6K PENDF (ENDF/B-VIII.1); H-in-H2O S(alpha,beta) (ENDF/B-VIII.1 tsl_H(H2O)_0001, 293.6 K) on all hydrogen with free-gas elsewhere, no self-shielding; recipe of layered-head-phantom collapse-v2.sh" \
  --output "$OUT"

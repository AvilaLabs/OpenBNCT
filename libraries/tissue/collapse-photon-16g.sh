#!/bin/bash
# Coupled photon data for the tissue library: photon-atomic collapse (16
# photon groups, 10 MeV down to 1 keV) bound to the 28-group TSL neutron data
# (multigroup-data-28g-tsl.json), for `sn photon-solve` / `openbnct project run`
# with [transport] photon_transport = true.
#
# Photon-atomic tables (photoatomic incoherent, coherent, photoelectric, pair)
# and the neutron-evaluation photon-production data come from the full
# ENDF/B-VIII.1 OpenMC HDF5 library (NOT the repo's selected/ subset, which
# has photon data for H/B/C/N/O only). The photon group edges are those of
# validation/fir1-k63-cylindrical-phantom/photon-collapse.sh. The neutron axis
# of the production matrix is pinned to the 28-group boundaries of the neutron
# data file. Charged-particle transport is not modelled: the kerma response
# deposits electron/positron energy where the photon interacts.
#
# Requires the external nuclear-data store (not in the repository); override
# with ENDFB81_HDF5 if it lives elsewhere. OPENBNCT selects the binary.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
LIB="${ENDFB81_HDF5:-/home/connoravila/nuclear-data/endfb-viii.1-hdf5}"
PBOUNDS="1e7,5e6,3e6,2e6,1.5e6,1e6,7e5,5.5e5,5e5,4e5,3e5,2e5,1e5,5e4,2e4,1e4,1e3"
OPENBNCT="${OPENBNCT:-openbnct}"
OUT="${OUT:-$ROOT/multigroup-photon-data-16g.json}"
MATERIALS="${MATERIALS:-air-dry water-liquid adipose-icrp soft-tissue-icrp muscle-skeletal-icrp brain-icrp skin-icrp blood-icrp lung-icrp lung-inflated-declared cortical-bone-icrp}"

MAT_ARGS=()
for m in $MATERIALS; do MAT_ARGS+=(--material "$ROOT/materials/$m.json"); done

"$OPENBNCT" sn photon-collapse \
  --photon-library "$LIB/photon" \
  --neutron-library "$LIB/neutron" \
  "${MAT_ARGS[@]}" \
  --photon-boundaries "$PBOUNDS" \
  --neutron-data "$ROOT/multigroup-data-28g-tsl.json" \
  --component-profile "$ROOT/component-profile-transported-photon.json" \
  --id openbnct.tissue-library.photon-data-16g.v1 \
  --note "Standard tissue library (PNNL-15870 Rev. 1 compositions); 16 photon groups 10 MeV-1 keV from the full ENDF/B-VIII.1 photon-atomic HDF5 library, n->gamma production from the ENDF/B-VIII.1 neutron HDF5 tables, bound to the 28-group multigroup-data-28g-tsl neutron structure; local charged-particle kerma, no bremsstrahlung or fluorescence" \
  --output "$OUT"

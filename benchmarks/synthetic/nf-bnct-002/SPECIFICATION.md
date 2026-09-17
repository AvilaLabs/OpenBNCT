# NF-BNCT-002: Deep-Penetration Heterogeneous Phantom

**Specification version:** 0.1.0

**Status:** Geometry, materials, heterogeneous assignment, and epithermal
source frozen; acceptance contract declared; execution pending

**Qualification ceiling:** Synthetic research only

## Purpose

`NF-BNCT-002` extends the frozen-case library into the regime NF-BNCT-001
does not cover: a deeper-penetration epithermal source into a larger,
materially heterogeneous phantom. It is designed to reveal:

- epithermal-spectrum source errors (energy-bin edge, weight, and
  normalization mistakes a monoenergetic source cannot expose);
- heterogeneous-material assignment errors (region ordering, voxel
  overlap, base-material fallback, density correction);
- thermalization-depth and deep-dose profile errors a 20 cm case
  understates; and
- boron-loading contrast errors (tumor vs normal-tissue capture).

It is not intended to model a clinical beam or patient. The epithermal
spectrum is a *declared* 1/E distribution — a benchmark input, not a
measured or facility beam.

## Canonical coordinate system and grid

- Patient-based right-handed LPS coordinates in millimetres; the same
  conventions as NF-BNCT-001.
- Grid `[60, 60, 60]` at `5.0 mm` spacing, origin `[-147.5, -147.5,
  -147.5] mm`: voxel boundaries are exactly `[-150, 150] mm` — a 30 cm
  cube, 1.5× NF-BNCT-001 per axis.
- For voxel index `(i, j, k)`, centre is
  `P_mm(i,j,k) = [-147.5 + 5i, -147.5 + 5j, -147.5 + 5k]`.

## Transport geometry and boundary

- Cube with boundaries `[-15, 15] cm` in x, y, and z.
- Vacuum boundary on all six faces.
- Heterogeneous materials per the assignment below; the base material
  fills every voxel outside declared regions.

## Materials

Three frozen materials; machine inputs in `transport/`:

| Artifact | Composition | Role |
| --- | --- | --- |
| `material-tissue-b10-10ugg.json` | ICRU four-component soft tissue, 10 µg/g B-10, `1.00000 g/cm3`, `293.6 K` | Base tissue |
| `material-tissue-b10-40ugg.json` | Identical tissue, 40 µg/g B-10 (the NF-BNCT-001 composition) | Tumor insert |
| `material-skull-equivalent.json` | Declared dominant-isotope bone: H-1 3.4%, C-12 15.5%, N-14 4.2%, O-16 43.5%, P-31 10.3%, Ca-40 23.1%, `1.85 g/cm3` | Skull slab |

The 10 µg/g tissue is derived from the NF-BNCT-001 composition by scaling
every non-B-10 nuclide by `0.99999/0.99996` and setting B-10 to
`0.000010`; fractions sum to one. The skull material is a declared
simplification — dominant isotopes only, no trace elements — chosen so a
backend cannot mistake it for a measured tissue. All materials use the
free-gas thermal treatment, matching the NF-BNCT-001 baseline.

## Heterogeneous assignment

`transport/assignment.json` (`openbnct.material-assignment/0.2.0`):

| Region | Voxels (inclusive `[i,j,k]` bounds) | World extent | Material |
| --- | --- | --- | --- |
| `skull_layer` | `[0,0,0]`–`[59,59,3]` | z ∈ `[-15, -13] cm` full cross-section | skull-equivalent |
| `tumor` | `[27,27,30]`–`[32,32,41]` | x,y ∈ `[-1.5, 1.5) cm`, z ∈ `[0, 6) cm` | tissue-B10-40µg/g |
| (base) | all remaining voxels | — | tissue-B10-10µg/g |

The skull slab sits against the incident face so epithermal neutrons
traverse dense bone-equivalent material before tissue — the heterogeneity
is on the beam axis, not decorative. The tumor box spans z ∈ [0, 6) cm
on axis, deep enough to sit past the thermal-fluence buildup.

## Source

`transport/source.json` (`openbnct.fixed-source-definition/0.1.0`):

- Fixed source; one unit-weight source neutron per history.
- Uniform disk of radius `8.0 cm` at z `= -14.999999 cm`, centered on
  the beam axis — just inside the incident face, same guard convention
  as NF-BNCT-001.
- Forward isotropic cone, half-angle `0.2 rad` about `+z`.
- Energy: declared `tabulated_histogram`, 16 log-spaced bins spanning
  `0.4 eV`–`20 keV`, bin weights ∝ `ln(E_hi/E_lo)` — a 1/E epithermal
  distribution. This is a declared benchmark spectrum; it asserts no
  facility measurement.

## Required component output and contract

Same output profile as NF-BNCT-001 (`openbnct.macroscopic-absorbed-dose.v1`,
component profile `openbnct.macroscopic-absorbed-dose.v1`), with the
boron component carrying the tumor/normal loading contrast.

The predeclared acceptance contract is
`transport/openmc-acceptance-contract.json`. Regions: `tumor` (single
bin, precision-gated), `axis_profile` (60 bins along z), and
`deep_point_20cm` (z ∈ [4.5, 5.5] cm inside the tumor, precision-gated).
Tolerances are relaxed relative to NF-BNCT-001 (ROI 2%, voxel-median 5%,
voxel-p95 10%) to reflect the deeper-penetration statistics; the gate
still demands the same estimator-closure and seed-replication checks.

## Execution status

No controlled execution yet. The case requires a response set bound to
the base material (per-material content binding — the NF-BNCT-001 sealed
set does not apply) and an NJOY HEATR receipt covering H-1, H-2, C-12,
C-13, N-14, N-15, O-16, O-17, O-18, B-10, P-31, and Ca-40. Deck
generation for the assigned geometry goes through
`openmc generate --assignment`.

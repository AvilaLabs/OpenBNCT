# Canonical Azmy problem (1988)

Internal transport verification against Azmy's quadrant-averaged
fluxes — the benchmark the weighted-diamond-difference literature is
scored against. Research-only qualification: algorithm verification
against published reference values, not a physics measurement.

## Problem

Azmy's weighted-DD problem (Azmy 1988) is a heterogeneous square with
a strong central source surrounded by a weakly scattering absorber —
it tests spatial differencing and quadrant-averaged accuracy where the
flux falls ~3 decades from source to corner.

Original formulation: quarter domain x,y ∈ [0,10] cm, mirror BCs on
x=0 and y=0, vacuum on x=10 and y=10:

| quadrant | extent | σ_t (cm⁻¹) | σ_s (cm⁻¹) | q (n/cm³/s) |
|---|---|---|---|---|
| lower-left  | x,y ∈ [0,5)     | 1 | 0.5 | 1 |
| lower-right | x∈[5,10], y<5   | 2 | 0.1 | 0 |
| upper-left  | x<5, y∈[5,10]   | 2 | 0.1 | 0 |
| upper-right | x,y ∈ [5,10]    | 2 | 0.1 | 0 |

We solve the **full square** x,y ∈ [−10,10] cm with vacuum on all four
faces and the source occupying |x|<5 ∩ |y|<5 — mirror BCs realized by
domain reflection (reflection equivariance is exact in the sweep, so
each mirrored family of quadrants is identical). z is a 0.5 cm
periodic slab, i.e. the infinite extrusion of the 2-D problem.

## Artifacts

- `generate.py` — emits `multigroup-data.json` (one group, two
  materials), `assignment.json` (single voxel_box for the source
  square, absorber base), `case.json` (UniformBox, q=1 n/cm³/s,
  total rate = box volume).
- `flux.json` — S8 solve output.
- `comparison.json` — quadrant means, relative errors, per-family
  symmetry spreads (self-consistency independent of the reference),
  verdict.
- `run.sh` — one solve + compare.

Grid: 80×80×2 cells (2.5 mm in plane); `AZMY_NXY` refines (must stay
a multiple of 4 so the |x|=|y|=5 cm boundary lies on cell edges).

## Scoring

Published quadrant means (Azmy 1988, and the anchors FeenoX's
`azmy-structured.fee` prints against):

| quadrant (quarter domain) | full-domain family | reference |
|---|---|---|
| lower-left  (source) | |x|<5,|y|<5          | 1.676   |
| lower-right (edge)   | 8 strips edge-adjacent | 4.159e-2 |
| upper-right (corner) | 4 corners              | 1.992e-3 |

Declared tolerances: source 2%, edge 5%, corner 10% (the corner mean
is ~1e-3 — the least-accurate absolute number for any discrete
method). For context, FeenoX's structured-grid S4 run reports
1.676 / 4.160e-2 / 1.991e-3 — our θ-WDD on a structured cell grid
should land at least that well.

Observed at the 2.5 mm mesh (S8, residual ~1.2e-5) — **verdict pass**:

| quadrant | ours | reference | rel |
|---|---|---|---|
| source  | 1.6789   | 1.676   | 0.17% |
| edge    | 4.1311e-2 | 4.159e-2 | 0.67% |
| corner  | 1.9151e-3 | 1.992e-3 | 3.9% |

Symmetry diagnostics: source and corner family spreads are exactly 0
and the eight edge strips agree to 1.7e-16 — mirror and x↔y-swap
equivariance hold to machine precision, so the quadrant means are
uncontaminated by directional-ordering artifacts.

`compare.py` also reports the per-family symmetry spreads — the
eight edge strips and four corners must agree to solver precision;
large spreads would flag a directional-ordering defect that the
quadrant means alone could mask.

## References

- Y. Y. Azmy, "The Weighted Diamond-Difference Form of Nodal
  Transport Methods," *Nuclear Science and Engineering* 98, 29–40
  (1988). DOI: 10.13182/NSE88-6.
- FeenoX `neutron_sn` example (azmy-structured), which reports the
  same three anchors and per-order convergence to them:
  <https://www.seamplex.com/feenox/examples/neutron_sn.html>

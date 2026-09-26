# Canonical Reed's problem (1971)

Internal transport verification against the classic heterogeneous
one-group slab benchmark, under the research-only qualification —
algorithm verification against a published reference solution, not a
physics measurement.

## Problem

Reed's problem (Reed 1971) is the standard test for spatial
differencing and angular transport in strongly heterogeneous media —
it combines a strong absorber/source, a void gap, and a c = 0.9
scattering/source region that is iteration-hard for source iteration.

Original half-slab, x ∈ [0,8] cm, reflecting BC at x = 0, vacuum at
x = 8:

| x (cm) | σ_t (cm⁻¹) | σ_s (cm⁻¹) | q (n/cm³/s) |
|---|---|---|---|
| 0–2 | 50 | 0   | 50 |
| 2–3 | 5  | 0   | 0  |
| 3–5 | ~0 | 0   | 0  |
| 5–6 | 1  | 0.9 | 1  |
| 6–8 | 1  | 0.9 | 0  |

This workbench has no reflecting boundary condition, so the problem is
solved on the **mirror domain** x ∈ [−8,8] cm: the region layout is
mirrored about x = 0 and vacuum applies at both ends — the solution on
x > 0 is the Reed solution exactly (reflection about a mirror BC is an
identity on the continuous problem, and on our θ-weighted diamond
difference it is equivariant to machine precision — verified by the
`uniform_box_partial_overlap` conformance test in
`openbnct-transport`).

The strong source box covers |x| < 2 in one `UniformBox`. The weak
scattering source occupies *two* disjoint slabs (5 < |x| < 6): the
solve places one box on [5,6] and `compare.py` adds the mirror copy by
x → −x symmetry (linearity — exact for the linear transport operator).

The void is declared σ_t = 1e-8 cm⁻¹ (the sweep requires finite σ_t;
1e-8 is 8 orders below the void's actual streaming physics).

## Artifacts

- `generate.py` — emits `multigroup-data.json` (one group,
  five declared materials), `assignment.json` (nine `voxel_box`
  regions; reflector is the base material), `case-src1.json`,
  `case-src2.json`.
- `reference/warsa-2002-eigenfunction.csv` — the eigenfunction
  expansion reference solution (Warsa 2002) as distributed by
  R. McClarren (`DrRyanMc/Benchmarks`, `ReedSol.csv`): 81 pointwise
  scalar-flux values on x ∈ [0,8] cm at 0.1 cm spacing.
- `run.sh` — runs both `sn solve`s and `compare.py`.
- `comparison.json` — emitted report: per-cell values, region-averaged
  table, error metrics, verdict.

Grid: 160 × 4 × 4 cells of 1 mm; periodic transverse (y,z) boundaries
realize the infinite slab; vacuum on both x faces. `REED_NX=320
REED_OUT=<dir>` regenerates the case at half spacing for the
refinement check (source boxes are declared in cm, region boxes in
cells — both follow the grid).

## Scoring

Two complementary metrics, both reported:

1. **Per-cell relative error** vs linear interpolation of the
   pointwise reference, restricted to cells with φ_ref > 0.05.
   Material-boundary cells are *not* scored this way: the reference
   CSV's 0.1 cm resolution cannot resolve the drop across the last
   absorber-adjacent cell (φ(1.9)=0.9995 → φ(2.0)=0.501), so pointwise
   interpolation inside boundary cells is itself unrepresentable — the
   cell-average-vs-interpolant mismatch there is a scoring artifact,
   not solver error. Interior cells ≥1 cell from a material boundary
   are the pointwise comparison's domain.
2. **Region-averaged scalar flux** over each material region
   (trapezoid-integrated reference on its own 0.1 cm grid), the
   quantity Reed's paper tabulates — robust to pointwise-interpolation
   artifacts in steep-gradient cells.

Acceptance (declared in `compare.py`): per-region tolerances —
2%/10%/1.5%/4%/1% for src1/abs1/void/src2/refl — plus interior
per-cell max 5%, RMS 2%. The absorber tolerance is looser because its
reference trapezoid is resolution-limited (see below).

Observed at the 1 mm mesh (S8, residual ~2e-5):

| region | ours | reference | rel |
|---|---|---|---|
| src1 (0–2)   | 0.9974 | 0.9875 | 1.01% |
| abs1 (2–3)   | 0.1597 | 0.1769 | 9.7% |
| void (3–5)   | 1.0966 | 1.1051 | 0.77% |
| src2 (5–6)   | 1.7186 | 1.7536 | 2.0% |
| refl (6–8)   | 0.7649 | 0.7666 | 0.22% |

Interior pointwise: max 2.3%, RMS 0.9%.

## Error budget (what the residual discrepancy *is*)

Mesh refinement 1 mm → 0.5 mm moves the region means by <0.3%
(src2 2.03→1.91%) — the error is **not** spatial truncation.
Quadrature order is the dominant term: src2's region mean moves
5.0% low at S4 → 2.0% low at S8 — ordinary S_N angular truncation
halving per order step, converging toward the eigenfunction
reference (the same pattern FeenoX documents for this problem).
The abs1 figure is additionally reference-limited: trapezoiding the
0.1 cm Warsa table over the steep convex dip overshoots the true
region mean, so the tabulated 9.7% upper-bounds the solver error.

## Convergence note

The c = 0.9 scattering regions make this problem iteration-slow for
plain source iteration (ρ ≈ 0.99): run solve B with a deep inner
budget (`--max-inner`, see `run.sh`). The within-group iterate settles
to a ~2e-5 relative-change floor set by the θ-WDD positivity clamps in
the near-void cells (the clamps switch on/off per iterate — the
transport map is mildly nonlinear there). The declared tolerances sit
above that floor; the physical flux is stable to ~1e-5 relative —
two orders inside the acceptance criterion.

## References

- W. H. Reed, "New Difference Schemes for the Neutron Transport
  Equation," *Nuclear Science and Engineering* 46, 309–314 (1971).
  DOI: 10.13182/NSE46-309.
- J. S. Warsa, "Analytical SN solutions in heterogeneous slabs using
  symbolic algebra computer programs," *Annals of Nuclear Energy* 29,
  851–874 (2002). Reference data: `ReedSol.csv` from
  <https://github.com/DrRyanMc/Benchmarks> (R. McClarren's distribution
  of Warsa's eigenfunction solution; the same table used by FeenoX's
  `neutron_sn` example for cross-verification).

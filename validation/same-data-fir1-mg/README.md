# Same-data OpenMC-MG ↔ S_N comparison — FiR-1 K63 cylindrical phantom

This directory holds the *same-data* cross-code experiment: OpenMC's
multigroup transport mode and the OpenBNCT S_N solver run the
**identical** 28-group constants over the same phantom, so the energy
condensation is common to both and any residual is attributable to
transport treatment alone.

- `convention_probe.py` — two-group manufactured system that pins the
  OpenMC conventions empirically before trusting the production run.
- `openmc_mg_run.py` — builds the OpenMC MG model from the committed
  data/case artifacts and runs a mesh-tally flux solve.
- `compare.py` — per-cell per-group S_N/MC ratios over the committed
  S₈ P1 flux artifact.
- `dose_fold_check.py` — independent NumPy fold of flux × declared
  dose-response vectors; audits the fold code against the committed
  dose bundle and folds the MC flux identically.

## Verified conventions (probe: convention_probe.py)

| convention | finding | evidence |
|---|---|---|
| `EnergyGroups` edges | **ascending** eV required; descending order silently produced "source energy above range" | probe run 2 |
| `XSdata.scatter_matrix` | `[G_in][G_out][l]` row-major — **same** orientation as `scatter_matrix_per_cm[g*G+g']`; launching g1-only source with `sm[0,1]=1` produced downscatter into g2 | probe run 3: `inout` gave lo=0.288/hi=0.451, `outin` gave lo=0 |
| scatter_format | `"legendre"` + `order` on `XSdata`; matrix needs a 3rd dim `[l]` | API probe |
| source energy | `openmc.stats.Discrete` takes **physical eV**, binned into groups internally — not a group index | `slab_mg` example + probe run 3 |
| `settings.tabular_legendre` | `{"enable": False}` required for legendre-format scatter data | `slab_mg` example |
| `XSdata.set_heating` | does not exist in OpenMC 0.15.3 — dose folding done independently in NumPy | API probe |

## Geometry conventions (verified against committed voxel_set)

The transport-case `origin_mm` is a **cell-centre** coordinate — the
domain edge is `origin − spacing/2` (multigroup.rs ~line 815). The
committed 24×24×26 @ 10 mm grid from centres (−115,−115,5) mm spans
x,y ∈ [−12,12] cm, z ∈ [0,26] cm. The water region is a cell-centre
rasterization of `ZCylinder(r=10, centre=(0,0))` over z ∈ [0,24] cm —
regenerating that rasterization reproduces the committed 7584-voxel
set exactly (316/layer × 24 layers).

Residual confound kept explicit: MC tracks the *analytic* cylinder;
S_N sees the rasterized set. Rim cells differ in filling fraction.
The mesh tally is laid on the S_N voxel edges so ratios compare
identical volumes.

## Source conventions

- Disk: r = 7 cm centred (0,0) on the z=0 face (the domain and the
  cylinder both start at z=0; birth is nudged 10 nm inside to avoid
  surface ambiguity).
- Angle: isotropic cone, half-angle 0.1491 rad about +z →
  `PolarAzimuthal(Uniform(cos θ_c, 1))`.
- Spectrum: 3-bin histogram spread into the 28 groups with the
  solver's `collapse_consistent` weighting (Maxwellian ∝ E·e^(−E/kT),
  kT = 0.0253 eV below 0.5 eV; 1/E above) — reimplemented verbatim in
  `spectrum_weight()`; per-group probabilities map to group-midpoint
  eV in a `Discrete` source.

## Runs

| run | dir | content |
|---|---|---|
| MC P1 10M | `target/mg-p1-10m-fixed/` | OpenMC MG, Legendre order 1, 10 batches × 1M, corrected geometry |
| MC P0 5M | `target/mg-p0-5m-fixed/` | OpenMC MG, isotropic scatter rows, 10 batches × 0.5M, corrected geometry |
| S_N P1 1cm | committed artifact | `multigroup-flux-28g-tsl-p1-1e.json` (converged, 122 outers) |
| S_N P1 5mm | `target/mg-fine/` | h-refinement arm — tests whether the near-face deficit is mesh truncation |
| S_N P0 1cm | `target/mg-p0-sn-1cm.json` | P0 arm — converged 90 outers, residual 9.65e-5 |

## Preliminary findings (corrected-geometry runs, 2026-09-27)

After fixing the geometry convention (origin_mm = cell-centre, not
edge; cylinder centred (0,0) over z∈[0,24]; mesh coincident with the
voxel grid) the comparison shows:

- **Fast groups (g0–g23) under P0: SN/MC ≈ 1.00–1.07** at every depth —
  beam normalisation, uncollided transport, and attenuation are right.
- **Near-face collided deficit ~0.7 under both P0 and P1, unchanged by
  h-refinement** (P1: 0.73/0.68 at z≤3 on both meshes) — a collided-field
  or source-coupling effect, not spatial truncation.
- **Deep thermal (g24–27): the anomaly.** MC's own P1/P0 ratio reaches
  ~2.5× at z≈12 (forward-peaked scatter penetrates deeper — correct
  physics). S_N instead produces *more* deep thermal under P0 than P1
  (SN-P0/SN-P1 ≈ 1.4–1.5 at z≈13–16): the anisotropy's effect on the
  deep field is inverted relative to MC.
- **P0 arm (transport correction OFF): SN/MC thermal rises to ~2.8× at
  z≈13** on the 1 cm mesh. Excess is uniform in radius (axis ≈ rim), so
  not a rasterisation edge effect.
- **Confound discovered**: `transport_correction` defaults ON under P0
  (`p0_transport_corrected`) — it strips the forward scatter lobe from
  σ_t, deepening penetration. Arms run under `OPENBNCT_NO_THETA_REPAIR`
  and `OPENBNCT_NO_CMR` accidentally resolved TC-on (~4.8–4.9× pileup),
  so their excess vs the 2.8× clean arm is the TC effect, not the
  disabled knob. The first 5 mm P0 solve was likewise TC-on (3.7×) and
  is discarded as an h-refinement arm.
- **Clean P0 h-refinement (TC off) — the pileup IS spatial
  truncation**: the corrected 5 mm arm (90 outers, residual ~1e-4,
  `flux-5mm-p0-notc.json`) halves the excess — thermal SN/MC peaks
  ~1.56 at z≈12 vs ~2.8 at 1 cm, near-face deficit improves 0.70 →
  0.85, epi mid-depth 1.2–1.3 → ~1.03–1.07, fast stays ~1.0. The
  mechanism is the DD/θ closure in optically thick thermal cells
  (σ_t·Δ ≈ 5 mfp at 1 cm) over-transmitting a declining field —
  first-order in h, consistent with the halving. A ~1.5× residual
  remains at 5 mm; S_N's diffusive-transport fidelity in the
  near-conservative thermal block converges toward MC but slowly.
- Tight convergence (1 cm, residual ~1e-6, 140 outers) left the 2.8×
  pileup unchanged — under-convergence ruled out as a cause.
- **P1 arm at 5 mm** (pre-uncollided-current fix): near-face deficit
  identical (0.65–0.73); deep field rises to ~1.0–1.17 at z≈9–15 vs
  ~0.80–0.90 on the 1 cm mesh — refinement *raises* the deep thermal
  toward/past MC.
- **P1 arm after the uncollided-current fix** (1 cm, `target/mg-p1-
  uncfix-1cm.json`, 82 outers): the anisotropy direction is now
  correct — S_N-P1 deep thermal exceeds S_N-P0 by ~2.2–2.4 vs MC's
  own P1/P0 ≈ 2.5. The epi band reproduces MC's mid-depth forward-
  scatter hump (MC-P1/MC-P0 ≈ 2.4 at z≈6; S_N-P1/S_N-P0 ≈ 3.4 —
  same shape, modestly amplified). Residuals: near-face thermal
  deficit ~0.71–0.74 persists unchanged, and the deep thermal SN/MC
  reaches ~3.0 at z≈15 — the same h-convergent pileup measured in the
  P0 arm, not a P1-specific defect.
- Balance: S_N P0 absorbed 0.245 vs MC-P0 (1 − 0.696) = 0.304 — S_N
  leaks ~5 pp more in P0; under P1 the committed artifact recorded
  absorbed 0.288 vs MC-P1 0.391.
- Upscatter verified working in OpenMC MG (2-group upscatter-only
  probe: g2→g1 flux split 0.500/0.500).

The thermal subsystem is near-conservative (σ_a ≈ 0.02–0.10/cm vs σ_t ≈
1.5–5.4/cm in g24–27 — ~50 collisions per thermal lifetime), so a few
per-cent bias in the per-cell transmission/loss balance compounds into
the observed factors. The remaining ~1.5× excess at 5 mm is consistent
with slow h-convergence of the same truncation (asymptotic transport
regime is only reached at τ_cell ≲ 1 mfp).

## Identified modelling gap (P1 arm) — FIXED 2026-09-27

Under `uncollided_split` + P1 the collided solve's first-collision
source deposited only `Σ_gp σ_s0(gp→g)·φ_unc(gp)` — the isotropic P0
in-scatter. The anisotropic first-scatter term
`3·Ω·Σ_gp σ_s1(gp→g)·J_unc(gp)` was never added: `current[cell]`
accumulated only the *collided* iterate's directional moments, and
`uncollided_beam_flux` returned scalar fluence — the beam's
first-scatter forward bias was dropped. MC-P1 samples the first
collision from the forward-peaked Legendre kernel, so S_N-P1
systematically under-penetrated (S_N/MC ≈ 0.7–0.85 through z ≈ 0–15).

**Fix** (`multigroup.rs`): the uncollided ray-trace now returns the
beam's directional moment per cell per group,
`J_unc[cell][g][a] = Σ_d Ω_{d,a}·φ_d` accumulated in the same deposit
loop as the scalar fluence (monodirectional ⇒ J = φ·Ω̂ exactly). The
P1 source fold uses the *total* current `J_collided + J_unc`, so the
beam's first scatter enters forward-peaked like MC. Two regression
tests pin the behaviour:
`multigroup::tests::uncollided_moments_track_beam_direction` (J = φ·Ω̂)
and `p1_uncollided_split_keeps_beam_anisotropy` (deep-thermal gain
1.82× with the term vs 1.02× with it forced off).

**Same-data rerun result**: the P1 solve's deep-thermal field tripled
at z ≈ 15 (fixed/old ≈ 3.3) and now *exceeds* the P0 arm by ~2.2–2.4×,
matching the direction and scale of MC's own P1/P0 ≈ 2.5 — the sign
inversion is resolved. What remains is the h-convergent thermal pileup
shared with the P0 arm and an unexplained near-face collided deficit
(~0.7) common to all beam-model arms.

**Dose consequence** (`dose_fold_check.py`, independent NumPy fold):
the fold itself refolds the committed bundle to 1e-16 exactly, so the
dose pipeline is clean — differences live in transport. Folding the
MC-P1 flux and comparing per-cell total dose: the committed (pre-fix)
P1 flux under-delivered deep dose (axis ratio SN/MC ~0.68–0.85 for
z ≥ 12; all-cell median 0.876). The fixed solve moves the all-cell
median to **1.016** and the beam axis to ~0.95–1.11 through z = 0–16.
Beyond z ≈ 17 the ratio dips to ~0.72–0.83 where MC statistics are
rel-std ≈ 0.3–0.6 and the residual truncation effect is largest.

| metric | before fix | after fix | MC reference |
|---|---|---|---|
| deep-thermal P1/P0 gain (2 cm slab, μ̄ = 0.7) | 1.02 | 1.82 | — |
| phantom P1/P0 deep-thermal ratio, z ≈ 12–15 | 0.69–0.75 (inverted) | 2.27–2.30 | ≈ 2.5 |
| epi mid-depth hump (S_N P1/P0, z ≈ 6) | 1.55 | 3.17 | 2.36 |
| same-data dose median vs MC-P1 | 0.876 | 1.016 | 1.0 |
| beam-axis dose z = 0–16 vs MC-P1 | 0.68–1.03 | 0.95–1.11 | 1.0 |

Outputs land in `target/` — they are working artifacts, not frozen
evidence; committed comparison results go in this directory only once
reviewed.

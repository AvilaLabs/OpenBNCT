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
- **Near-face thermal deficit — partially explained and partially
  fixed.** Two stacked defects shared the symptom. (a) *Uncollided
  deposit under-sampling (fixed)*: the uncollided ray-trace evaluated
  e^{−σs} at the cell's axial centre instead of the cell mean along
  the in-cell ray segment — under-depositing by
  `e^{−σΔ/2}·(σΔ)/(1−e^{−σΔ})`: −1% at σΔ = 0.5, −9% at 1.5, −30% at
  3, −64% at σ_t·Δ = 5.4 (deepest thermal group at 1 cm). The
  segment-mean is now closed-form (`uncollided_beam_flux`), with the
  `uncollided_deposit_is_axial_cell_mean` regression test asserting
  the exact mean at σΔ = 3. Post-fix P0 1 cm arm: near-face thermal
  SN/MC 0.70 → **0.80/0.75/0.82/0.92** at z = 0–3 (all radial bins
  inside the beam improved ~8–15%); epi band near-face is *not*
  deficient (0.97–1.17, mid-depth hump 1.3–1.4 as before).
  (b) *Collided boundary-layer truncation (open)*: a thermal-specific
  residual ~0.75–0.85 persists at z ≤ 2 — the collided sweep's
  first-cell treatment of the steep face-weighted first-scatter
  source (flat in-cell source + DD closure), h-convergent like the
  deep-thermal pileup (the 5 mm arm, where the deposit error was
  already negligible, sits at ~0.85). Not noise: MC rel-std ≈ 0.4%.
- **`boundary_flux` is a null reference for this beam**: a P0 arm run
  with the beam inside the collided solve (95 outers) collapses the
  thermal band to SN/MC ≈ 0.06–0.23 — the 8.5° cone smeared over S8
  ordinates cannot reproduce a narrow beam's penetration (the
  uncollided split exists precisely for this reason). Documented as a
  mode limitation, not a comparator.
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
shared with the P0 arm and a partly-explained near-face collided
deficit (see the near-face entry in the findings above).

**Second fix — uncollided deposit is now the axial cell mean.** The
ray-trace deposited e^{−σs} evaluated at the cell's axial centre; the
cell-mean along the ray segment is
`e^{−σ·s_lo}(1−e^{−σ·span})/(σ·span)` (closed form, face-clipped) —
the centre value under-counts by ~`(σΔ)²/24`, i.e. 9% at σΔ = 1.5,
30% at 3, 64% at 5.4. The fix covers the `beam_sigma_nodes` kernel
path identically (per-node segment means). Same-data effect (P0 1 cm):
near-face thermal SN/MC 0.70 → 0.80–0.92 at z ≤ 3, all radial bins
inside the beam improving ~8–15%; fast and epi bands unchanged; the
deep pileup is untouched (grows to ~3.1 as the now-correct source
feeds it) — confirming the two anomalies were always separate.
Residual ~0.75–0.85 thermal-only deficit at z ≤ 2 is the collided
sweep's boundary-layer truncation documented in the findings.

**Dose consequence** (`dose_fold_check.py`, independent NumPy fold):
the fold itself refolds the committed bundle to 1e-16 exactly, so the
dose pipeline is clean — differences live in transport. Folding the
MC-P1 flux and comparing per-cell total dose: the committed (pre-fix)
P1 flux under-delivered deep dose (axis ratio SN/MC ~0.68–0.85 for
z ≥ 12; all-cell median 0.876). With both fixes the all-cell median
is **1.019** and the beam axis runs ~1.00–1.03 at z ≤ 5, drifting to
~0.79–0.93 at z = 6–16. Beyond z ≈ 17 the ratio dips to ~0.58–0.75
where MC statistics are rel-std ≈ 0.3–0.6 and the residual truncation
effect is largest. The boron/photon dose components (thermal-driven)
sit at ~2.3× MC — the deep-thermal pileup mapped through the dose
responses, consistent with the flux-level finding.

| metric | before fixes | after fixes | MC reference |
|---|---|---|---|
| deep-thermal P1/P0 gain (2 cm slab, μ̄ = 0.7) | 1.02 | 1.82 | — |
| phantom P1/P0 deep-thermal ratio, z ≈ 12–15 | 0.69–0.75 (inverted) | 2.27–2.30 | ≈ 2.5 |
| epi mid-depth hump (S_N P1/P0, z ≈ 6) | 1.55 | 3.17 | 2.36 |
| near-face thermal SN/MC, z ≤ 3 | 0.65–0.73 | 0.79–0.94 | 1.0 |
| same-data dose median vs MC-P1 | 0.876 | 1.019 | 1.0 |
| beam-axis dose z = 0–5 vs MC-P1 | 0.68–1.03 | 1.00–1.03 | 1.0 |

Outputs land in `target/` — they are working artifacts, not frozen
evidence; committed comparison results go in this directory only once
reviewed.

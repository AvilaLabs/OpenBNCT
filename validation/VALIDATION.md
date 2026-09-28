# Validation dossier

How OpenBNCT is checked, what is actually verified, and what is
honestly not yet. Research software only — nothing here is clinical
qualification, equivalence, commissioning, or regulatory evidence.
See `docs/DISCLAIMER.md`.

The program follows the standard V&V split:

- **Code verification** — does the implementation solve the intended
  equations correctly? (manufactured solutions, invariants, option
  matrices, regression pins)
- **Solution verification** — do discretization and iteration errors
  behave as the method requires? (h-refinement order, canonical cases)
- **Validation** — does the modeled system agree with measurements?
  (FiR-1 phantom data, microdosimetry)
- **Intercomparison** — does an independent code agree on identical
  inputs? (OpenMC MG cross-checks)

## 1. Per-commit verification layer (CI-enforced)

Runs under `cargo test --workspace --all-targets` on every push.

| Layer | Coverage | Result |
|---|---|---|
| MMS h-refinement | Exponential manufactured field on a periodic-transverse column, S4, θ-WDD; N = 8→128 | L2 error 5.4e-3 → 2.8e-5; observed order climbs 1.71 → 1.99 → asymptotic 2.0 (design rate) |
| Option matrix — infinite-medium oracle | 32 flag combinations (Anderson × CMR × θ-repair × transport-correction × P1) | Every combination reproduces the analytic fixed point to 1e-8 relative; the transport-corrected equation matches its own corrected analytic solution; σ_a preserved exactly (balance = 1.0); P1+TC declaration verified to suppress the correction (no double-counted forward lobe) |
| Option matrix — leaky beam | 64 flag combinations (adds uncollided-split/boundary source paths) | All converge; all fluxes non-negative; balance audit ≤ 1 (no fabricated particles); fixed-point-preserving accelerators agree within each physics class to the convergence floor |
| Bug-class regressions | Wrap-cycle one-step convergence; P1 realizability bound `|σ_s1| ≤ σ_s0`; coverage/volume-scaled deposit; θ-repair non-fabrication | Pinned — see test list below |
| Parser rejection corpus | DICOM PET (truncation, magic corruption, bit flips, wrong modality, malformed composition); ENDF MF3/MF7/MF33 mutated tapes; evaluated-source contract (unknown/missing fields, type confusion, non-canonical digests, bad enum, number overflow); content references (digest canonicality, namespace normalization) | All malformed inputs rejected with diagnostics or verified no-panic paths |

Regression pins (the bug classes they guard):

- `periodic_near_conservative_converges_one_step` — the shared-wrap
  cross-group contamination that produced a period-2 orbit on
  near-conservative periodic problems. The solver now records
  `convergence_mode` on every flux artifact: `parity_midpoint` means
  the iterate settled into a period-2 orbit and the emitted field is a
  projection; absent means ordinary one-step convergence.
- `scatter_p1_bounded_by_p0_row` — data-side realizability rejection.
- `uncollided_deposit_scales_by_transverse_coverage`,
  `uniform_box_partial_overlap_scales_by_volume` — source-deposit
  coverage regression (the per-cell area-fraction bug class).
- `theta_repair_never_fabricates_particles` — the θ-clamp defect
  channel of the particle-balance audit.

## 2. Frozen evidence layer (`bench verify`, CI-enforced)

`openbnct bench verify --catalogue benchmark-catalogue.json --root .`
re-hashes every declared artifact and checks every claim reference;
`openbnct qual verify` checks the qualification record against the
catalogue. Both run in CI. Frozen evidence under `validation/` is
immutable — it is never regenerated in normal CI.

### Canonical / intercomparison cases

| Case | Reference | Verdict | Limitation |
|---|---|---|---|
| canonical-reed-problem | Warsa (2002) eigenfunction expansion, 81 pointwise fluxes | PASS — region means ≤1% flat, ~2% scattering peak; interior RMS 0.9% | Angular truncation error is real and documented (S4→S8 halving) |
| canonical-azmy-problem | Published quadrant means | PASS — 0.17% / 0.67% / 3.9% | — |
| canonical-kobayashi-p1 | Analytic ray integral + GMVP MC table | PASS near field ≤5%; deep field documents ray effect by design | S_N ray effects are **reported**, not graded — deep probes overshoot the MC field (−40% off-lobe, +110–218% diagonal) — this is the benchmark's purpose |
| intercomparison-kobayashi-p1 | OpenMC multigroup, same inputs | MC matches GMVP ~1σ; S_N–MC deltas field-resolved | Difference is the deterministic truncation signature, not a data defect |
| intercomparison-layered-head | OpenMC | Executed | — |

### Measured-data anchors

| Case | Reference | Verdict | Limitation |
|---|---|---|---|
| fir1-k63-cylindrical-phantom | Digitized TECDOC-1223 FIG. 3 water series | PASS — 12/12 bins ≤ ~1.3σ, χ² = 6.7 | Digitized-plot provenance; 3-group collapse |
| fir1-k63-pmma-phantom | Digitized TECDOC-1223 FIG. 3 PMMA series | PASS — 15/15 bins ≤ ~1.15σ, χ² = 6.0 | Same |
| fir1-k63-water-phantom | Published advantage depth / thermal max (Seppälä 2002) + digitized profile | Executed — shape comparison | Phantom geometry differs from the measured cylinder; grade is shape-level |
| fir1-k63-liquid-b-phantom, fir1-k63-pmma-pk-anchors, fir1-k63-scenario-budget | FiR-1 family | Executed | — |
| cell-microdosimetry-sato2018 | Sato (2018) BPA/BSH parameterizations | Executed | — |
| pg-benedicte-geometry | BeNEdiCTE/LENA-class literature geometry | Executed | — |

### Honest open entries

| Entry | Status | Note |
|---|---|---|
| nf-bnct-001 | **candidate** | Synthetic DICOM chain verified end-to-end (synth-PET → SUV → boron → tiered assignment); not a measured-beam validation |
| nf-bnct-002 | **unexecuted** | No evidence claims; do not read as validated |

## 3. Claims register (qualification-record.json)

`qual verify` binds each claim to catalogue evidence. Statuses:

- **held**: q1 canonical profiles; q2 component dose vs continuous-energy MC (layered head); q3 FiR-1 phantom dose-depth; q4 robust weight optimization; q5 Sato microdosimetry; q6 prompt-gamma geometry; q7 correlated uncertainty propagation.
- **external_dependency**: q8 independent external reproduction; q9 clinical/facility treatment-device qualification.
- **planned**: q10 prospective partner phantom run.

## 4. Known limitations — declared, not hidden

1. **Energy-condensation residual**: on the water-column comparison
   the collapsed 56-group data produces deep over-moderation (epi
   pileup ~10× at 15 cm in the 0.2 eV–keV shoulder) with deep fast
   under-transport. This is a collapse artifact, not a sweep defect —
   the MMS table above and the intercomparison cases bound the sweep.
   Weighted recollapse experiments (deep/volume/survival weightings)
   all over-penetrate; P1 anisotropy removes the epi pileup but
   under-transports deep. The residual is open.
2. **S_N ray effects** in void-like deep-field geometry (Kobayashi) —
   documented quantitatively rather than graded.
3. **Single measured facility anchor** — FiR-1 is the only measured
   beam; a second facility (RA-6/MIT-class published depth profiles)
   is gated on public tabulated data availability.
4. **No external reproduction yet** (q8), no clinical qualification
   (q9) — by design; both are external dependencies, not open code work.
5. **Digitized-provenance data** — TECDOC-1223 series are digitized
   plots, not tabulated measurements; grade accordingly.

## 5. Reproduce

```sh
# Per-commit verification (unit tests, MMS, option matrices, corpus)
cargo test --workspace --all-targets

# Frozen evidence integrity + claim bindings
openbnct bench verify --catalogue benchmark-catalogue.json --root .
openbnct qual verify --record qualification-record.json \
    --catalogue benchmark-catalogue.json

# Individual canonical cases (each dir has run.sh + compare.py)
# e.g. validation/canonical-reed-problem/run.sh
```

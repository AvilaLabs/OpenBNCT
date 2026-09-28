# Validation program handoff: same-data S_N vs OpenMC, FiR-1 phantom

Prepared 2026-10-21 for the next engineering instance. Implementation brief +
state record for the validation/dosimetry-fidelity workstream. The transport
code work is committed on `main` (HEAD `a1aa9ad`); this document covers what
was built, what the measurements show, and the two open residuals with
concrete discriminators for each.

## Ground rules (non-negotiable, from AGENTS.md)

- Only the coordinating agent runs builds/tests/transport jobs; run ONE
  bounded job at a time inside the systemd cgroup:
  `systemd-run --user --scope -p MemoryMax=6G -p MemorySwapMax=0 \
   -p TasksMax=128 -p CPUQuota=200% -- env CARGO_BUILD_JOBS=1 \
   RUST_TEST_THREADS=1 RAYON_NUM_THREADS=2 TMPDIR=$PWD/target/preflight-tmp \
   COMMAND ...` — if the cgroup cannot be enforced, stop.
- `TMPDIR` must be disk-backed (`target/preflight-tmp`), not `/tmp`.
- Frozen evidence under `benchmarks/` and `validation/` is immutable — do
  not regenerate or "improve" committed artifacts. Working artifacts live in
  `target/` and are explicitly *not* evidence.
- Commits: plain messages, no AI attribution/trailers/links. Push to `main`.
- Research-only scope. No clinical/equivalence/regulatory claims.
- CI gates: `cargo fmt --all` then `cargo fmt --check`, and
  `cargo clippy --workspace --all-targets -- -D warnings`. Both clean at HEAD.
- Multiple agent sessions run on this laptop — do not kill or reconfigure
  other assistants' processes.

## What the validation program is

`validation/same-data-fir1-mg/` — a FiR-1 BNCT cylindrical water phantom
(`validation/fir1-k63-cylindrical-phantom/`, 24×24×26 cells, 1 cm, 28-group
collapsed data `multigroup-data-28g-tsl-v4.json`) solved by BOTH the S_N
solver (`openbnct sn solve`) and OpenMC's multigroup mode on the *same*
collapsed data file — so any discrepancy is discretization/model, not
cross-section condensation. Reference: OpenMC `cell_flux` mesh tally, 10
batches × 1M particles, P0 arm `target/mg-p0-5m-fixed/statepoint.10.h5`,
P1 arm `target/mg-p1-10m-fixed/statepoint.10.h5`.

Read `validation/same-data-fir1-mg/README.md` first — it is the dossier with
the full finding history. The comparison scripts:

```bash
# uses the openmc-enabled venv:
VENV=/home/connoravila/.venvs/w003env/bin/python

# band-resolved SN/MC ratio table (fast/epi/thermal × z-slab):
$VENV validation/same-data-fir1-mg/compare.py \
    target/mg-p1-10m-fixed/statepoint.10.h5 <sn-flux.json> <out.json>

# dose fold SN-vs-MC through the declared response vectors:
$VENV validation/same-data-fir1-mg/dose_fold_check.py \
    <statepoint.h5> --dose <dose.json> --flux <sn-flux.json>
```

S_N solve command for a comparison arm (release build, ~50 s/outer under
the cgroup; P1 converges ~88 outers, P0 ~97):

```bash
SN_OUTER_DEBUG=1 target/release/openbnct sn solve \
  --case validation/fir1-k63-cylindrical-phantom/case.json \
  --assignment validation/fir1-k63-cylindrical-phantom/assignment.json \
  --data validation/fir1-k63-cylindrical-phantom/multigroup-data-28g-tsl-v4.json \
  --order 8 --p1 --convergence 1e-4 --max-outer 400 --allow-unconverged \
  --output target/<name>.json
```

(`--p1` only on the P1 arm; drop `--allow-unconverged` if you want a hard
failure instead of a written field on cap. `SN_OUTER_DEBUG` prints a
per-outer residual line; `SN_INNER_DEBUG` prints inner convergence.)

## State of agreement vs OpenMC (1 cm mesh, S8)

| metric | status |
|---|---|
| P0 deep thermal z=12–21 | **0.96–1.10** (was 2.5–2.8× pileup pre-exp_source) |
| P0 mid-depth thermal | ~1.00–1.02 |
| P0 epi/fast | ~1.0 (fast), ~1.02 mid epi |
| P1 deep thermal z=12–16 | ~1.24–1.44 — **open residual** |
| P1 epi mid-depth hump z=4–9 | ~1.39–1.50 — **open residual** |
| P1 near-face thermal z≤3 | ~0.90–1.15 (was 0.79–0.94) |
| dose fold vs MC-P1 | median 1.0184, axis 0.93–1.12 (z≤16) |
| thermal dose components (B,N,γ) | ~1.33× (was ~2.3×) |
| convergence | P0 97 / P1 88 outers at residual ~1e-4 |

Working artifacts (target/, regenerable): `mg-p0-linsrc-1cm.json`,
`mg-p1-linsrc-1cm.json`, `mg-p1-dirlam-1cm.json` + `-comparison.json`.

## What was fixed in this workstream (all on main)

- `01cd5cf` P1 uncollided-beam current feeds the first-scatter dipole
  (`uncollided_current` → `p1_source`); without it the beam's first scatter
  was isotropized — deep P1 field was ~3× low. Includes the validation
  dossier + parser rejection corpora.
- `51c2860` uncollided deposit is the axial cell-MEAN of e^{−σs} along the
  ray segment (was center-point — ~30 % under-source at σΔ=3). Near-face
  thermal deficit improved.
- `364bfc8` exponential within-cell source (`SnOptions.exp_source`, σΔ>1
  gate): fixes DD's perfect-mixer pathology — thick near-conservative cells
  transmitted the cell mean to the outflow edge, over-transporting a
  declining tail (probe: ~1.47×/cell runaway → flat ~1.31 amplitude).
  Eliminated the P0 deep-thermal pileup. Sub-discoveries baked in:
  λ is rebuilt per inner-iteration series (inner map stays affine) and
  frozen after a 16-outer warmup (λ↔φ lag otherwise sustains a period-2
  oscillation). `OPENBNCT_NO_EXP_SOURCE` A/Bs it.
- `a1aa9ad` per-component direction-aware λ: under P1/l≥2 the dipole
  (3Ω·p1) and kernel components get their own per-direction rates fit on
  their own signed fields; NaN marks degenerate triplets → flat share.
  `OPENBNCT_NO_DIR_LAMBDA` A/Bs the directional fits.

## Diagnostics worth knowing (all env kill-switches, A/B only)

`OPENBNCT_NO_EXP_SOURCE`, `OPENBNCT_NO_DIR_LAMBDA`,
`OPENBNCT_NO_THETA_REPAIR`, `OPENBNCT_NO_CMR`,
`SN_OUTER_DEBUG`, `SN_INNER_DEBUG` (in `multigroup.rs`; read once at solve
start except the debug prints).

## The controlled probe (the discriminating instrument)

`thick_cell_conservative_column_probe[_p1]` in `multigroup.rs` tests:
a 1-group near-conservative periodic column (σ_t=3, σ_s=2.85/cm, μ̄=0.7
forward scatter for the P1 arm), 24×1cm cells vs a 1mm fine solve — pure
within-cell-source error, nothing else. P0 probe: corrected tail flat
~1.315 (amplitude offset, not rate); legacy runaway >50× asserted. P1
probe: bounded ~1.46 tail (dir-fit) vs ~1.8 (dir-free) vs 13.5× (legacy).

## OPEN ITEM 1 — P1 epi mid-depth hump (~1.4–1.5 at z=4–9)

**Evidence chain:**
- P1 thermal excess tracks the epi hump — thermal in-scatter is sourced
  by φ_epi, so a +40% epi field produces a +40% thermal source. The deep
  closure cannot remove an upstream error.
- The hump cells have σ_t·Δ ≈ 0.5–1.5 — at or below the exp_source gate
  (σΔ>1), so the closure never engages there.
- The dir-aware arm changed the deep-cell closure but the phantom field
  did not move → residual is NOT the deep-cell reconstruction.
- P0 epi band is flat ~1.02 → the hump is P1-specific.

**Candidate mechanisms (unranked):**
1. Flat-source error surviving in the sub-gate (0.5<σΔ<1) epi cells —
   the gate can't simply be lowered (MMS order test needs a uniform
   method per cell; a cell-thickness gate keeps it per-cell-uniform, but
   engaging at σΔ≤1 corrupts the MMS level where σ_t·Δ crosses 1).
   A *smooth* blend σΔ→weight might preserve order — needs care.
2. S_N deterministic dipole source vs OpenMC's sampled-P1 kernel — a
   genuine angular-model difference that h-refinement cannot remove.
3. P1 uncollided-current first-scatter weighting at mid-depth.

**Discriminator to run first:** the 5mm P1 arm (`target/mg-fine/`
assignment/case exist; old flux is stale — regenerate). If the hump
shrinks ~2× at 5mm → discretization (mechanism 1); if it persists →
model difference (mechanism 2) → document, don't chase the sweep.
A cheaper 1-D discriminator: an epi-band column probe with σΔ~0.7 cells
and a P1 source — check whether the ungated band shows a hump-like drift.

## OPEN ITEM 2 — near-face thermal deficit (~0.85–0.9 at z≤2)

Collided boundary-layer truncation — the entry layer's first-scatter
deposit is cell-mean smeared; h-convergent (the 5mm arm improved it).
Separate mechanism from the epi hump. Likely closeable by a refined
deposit (sub-cell first-collision quadrature along the ray) — bounded
improvement expected, diminishing returns at 1 cm mesh.

## Engineering notes

- The λ cache (`SourceRecon`) is per group, rebuilt at `inner_iter == 0`
  for the first 16 outers (`LAMBDA_WARMUP`), then frozen — DO NOT move
  the rebuild inside the inner sweep (≈6× slowdown, cache thrash, and a
  λ↔φ period-2 limit cycle). Verified painful; the comments explain why.
- `cmr_preserves_the_transport_fixed_point` pins `exp_source: false` on
  both arms — CMR's ideal-exchange model doesn't include the λ share,
  so enabling it shifts the fixed point ~4e-5 during warmup. Deliberate.
- θ repair is skipped on signed-source cells (P1/l≥2 present) — negative
  angular flux is legitimate physics there.
- Level-symmetric S8 = 80 ordinates; per-direction λ fields under P1 are
  ~29 MB/group transient during warmup (80×14976×3×8B) — fine under 6G.
- Test runtimes: full transport lib suite ≈ 15 min bounded; both probes
  ≈ 2.5 min each; use name filters, never an unrestricted run.
- The `w003env` venv (`/home/connoravila/.venvs/w003env/bin/python`) has
  openmc 0.15.3 for statepoint reading; system python3 lacks it.
- `--anderson 4` exists but the λ-warmup made it unnecessary (Anderson
  perturbed the period-2 orbit and stalled parity convergence — prefer
  the frozen-λ plain map).
- `validation/fir1-k63-cylindrical-phantom/` artifacts marked committed
  are frozen: `multigroup-flux-28g-tsl-p1-1e.json`, `dose-28g-p1-1e.json`,
  `measurement-comparison-cylindrical-28g-p1-1e.json`,
  `beam-quality-cylindrical-28g-p1-1e.json` (the committed flux artifact
  is the pre-exp_source P1 solve at 122 outers — historical record;
  superseding it requires a reviewed evidence refresh, not a silent swap).

## If you take up open item 1

1. Regenerate the 5mm P1 arm with exp_source — `target/mg-fine/case-5mm.json`
   + `assignment-5mm.json` exist (`flux-5mm-p1.json` there is pre-exp_source,
   stale); ~2–3h bounded. Then `$VENV validation/same-data-fir1-mg/
   compare_fine.py target/mg-p1-10m-fixed/statepoint.10.h5
   target/mg-fine/flux-5mm-p1.json` — it coarsens the 5mm field onto the
   1cm MC mesh (no 5mm MC run needed; MC transport is mesh-free).
2. If the hump persists: read OpenMC's MG P1 scattering docs/code — the
   sampled f(μ) from l≤1 moments can carry partial-negativity handling
   that differs from the deterministic dipole source.
3. If it shrinks: the fix is engaging the correction at σΔ<1 without
   breaking `mms_sweep_converges_at_design_order` — likely a blended
   weight w(σΔ) rather than a hard gate; MMS uses σ_t·Δ=1.0→0.06 so any
   engagement below 1 must be second-order-consistent.
4. Whatever you conclude, update the README table + this file.

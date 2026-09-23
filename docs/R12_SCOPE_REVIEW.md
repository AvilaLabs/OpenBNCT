# R12-01 — Avify Dose integration: scope and IP-boundary review record

**Status:** DRAFT — prepared for owner review. The roadmap requires a
recorded review before patent-sensitive design, implementation, or public
technical disclosure; this document is the record once the owner signs off.
Until then it is a working draft and nothing in R12-02..06 may start.

**Scope sources (non-confidential to the owner; all local):**

- `avify-dose-paper` — public paper reproducibility package (method,
  envelope construction, declared sets, certificate contents).
- `Project-Waddle` — internal development repository: protocols
  W001–W007, `verifier/` engine, `filing/` provisional specification
  (US provisional 64/132,705).
- `docs/IP_BOUNDARY.md` — the standing boundary this review applies.

## 1. What Avify Dose is (for connector purposes)

The engine is `Project-Waddle/verifier/plan_verify.py` (with
`plan_model.py`, `ingest_dicom.py`, `ingest_source.py`): a self-contained
Python + OpenMC pipeline that

1. ingests a voxel plan (class map + ROI masks + grid metadata),
2. takes a plan JSON declaring the uptake-uncertainty set, biological
   weights, ROI criteria, normalisation, run budget, and beam,
3. constructs the corner boron maps **inside the engine** and runs two
   (optionally three) OpenMC fixed-source evaluations,
4. constructs the per-ROI dose envelopes (component bounds: boron
   mixed-product, nitrogen endpoint, fitted photon response, fast
   endpoint-mean with allowance; 3σ expansion) and
5. emits a hash-bound `certificate.json` carrying per-ROI envelope
   edges, uncertainty, applicability flags (photon-validity, fast
   allowance), per-criterion actions (PASS / FAIL /
   ADDITIONAL_EVIDENCE), run records, and full provenance hashes.

The envelope construction, extremal-map choice, applicability checks,
and certificate assembly are the patent-sensitive core and remain inside
the separately licensed engine.

## 2. Interchange contract (measured from the implementation)

**Input — voxel plan** (`<prefix>_arrays.npz` + `<prefix>_meta.json`):

- `cls`: integer class index per voxel, shape (nz, ny, nx)
- `roi_<name>`: boolean ROI masks (tumour, brain, scalp in the recorded
  domain)
- meta: `classes` list, `lower_left_cm_xyz`, `voxel_cm`,
  `density_g_cm3` per class
- material domain: the engine's tissue classes (air, brain, cranium,
  scalp, tumour); tumour = brain base material plus boron loading.

**Input — plan JSON:**

```json
{
  "declared_set": {"rt": [lo, nom, hi], "rs": [lo, nom, hi], "B": [lo, nom, hi]},
  "brain_ratio": 1.0,
  "weights":  {"tumour": [wB,wN,wf,wg], "brain": [...], "scalp": [...]},
  "criteria": {"tumour": [">=", 20.0], "brain": ["<=", 11.0], "scalp": ["<=", 10.0]},
  "normalisation": {"source_particles_total": N}
                 | {"fluence_rate_cm2s": f, "area_cm2": a, "time_s": t},
  "histories": {"corner": 4e7, "nominal": 2e7},
  "seeds": {"lo": 1, "hi": 2, "nom": 3},
  "beam": "epithermal" | {"file_source": "path.h5"} | {"spectrum": {...}}
}
```

**Output — `certificate.json`:** input/ingest/harness/verifier SHA-256
hashes, parameters used, per-run records (ppm map, seed, histories,
wall-clock seconds, tallies, atom densities, ROI masses), per-ROI
brackets `{L, U, sL, sU, gammaH, gammaB, photon_valid, fast_ok}`, direct
doses, the Gy-w normalisation scale, and per-ROI actions.

## 3. IP-boundary analysis (what the connector may contain)

`IP_BOUNDARY.md` requires review before implementing: continuous
admissible boron-map optimization, extremal-map construction,
designated transport-run recomposition, robust dose-bound certification
and certificate checking.

Under the measured contract, **every claimed step lives inside the
engine**. The OpenBNCT-side connector therefore consists only of:

| Connector component | Claimed step? | Basis |
|---|---|---|
| Export case → voxel plan (npz + meta) | No — mechanical format conversion of geometry/classes |
| Assemble plan JSON (declared set passed verbatim) | No — the engine computes the corner maps itself (`ppm_map` inside `plan_verify`); OpenBNCT must NOT precompute extremal maps |
| Beam mapping (case beam → engine beam spec) | No — parameter passing; spectrum/histogram serialization only |
| Subprocess orchestration (bounded, cancellable) | No — infrastructure; must satisfy AGENTS.md process rules |
| Ingest + display certificate.json | No — rendering the engine's output; no re-derivation of envelopes |
| Timing/work reporting (R12-05) | No — bookkeeping over run records the engine returns |

**Explicitly excluded from the MIT tree:** corner-map construction,
envelope/bracket arithmetic, applicability checks, certificate
derivation or verification. If a future design needs OpenBNCT to
recompute any of these, this review must be revisited.

## 4. Supported domain (v1 constraints implied by the engine)

- ROI names fixed: tumour, brain, scalp (engine tally/weight schema).
- Material classes fixed: air, brain, cranium, scalp, tumour — an
  OpenBNCT case must reduce to these classes per voxel (majority or
  declared rule); richer assignments are out of v1 scope.
- Beam forms: the built-in epithermal preset, an OpenMC file source,
  or a spectrum dict. OpenBNCT beam → spectrum export is the v1 path.
- Engine transport is OpenMC inside the engine process; OpenBNCT's
  solver is not in the verification path (that also keeps the MIT tree
  clear of run-recomposition claims).
- Output semantics: empirical two-evaluation envelopes with recorded
  applicability flags — **not** certified total-dose bounds, not
  clinical plan acceptance (per the paper's own scope).

## 5. Open questions for the owner

1. **Invocation form.** Is `Project-Waddle/verifier/` the integration
   artifact (invoked as a script), or should R12-02 assume a packaged
   Avify Dose CLI/API? This decides the connector's process contract.
2. **Material mapping.** Confirm the v1 restriction to the five engine
   classes is acceptable, or specify the mapping for additional classes.
3. **ROI generality.** v1 to the recorded three ROIs, or does the
   engine already support arbitrary ROI sets?
4. **Surface order.** CLI-first (`openbnct avify …`) or GUI workspace
   first (R12-03)? CLI-first is the smaller lift and exercises the same
   contract.
5. **Certificate display.** Which fields surface in the GUI, and how is
   the "empirical envelope — not certified" status rendered (R12-04)?

## 6. Owner sign-off

- [ ] I/O contract above confirmed against the engine
- [ ] Boundary table §3 approved — connector contains no claimed step
- [ ] Open questions §5 answered
- [ ] Approved to proceed to R12-02 (connector design)

**Reviewer:** Connor Avila — **Date:** ____

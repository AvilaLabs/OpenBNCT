# SINBAD shielding benchmarks — acquisition-gated scaffold

SINBAD (Shielding Integral Benchmark Archive and Database) is the
field-standard experimental anchor for neutron shielding transport —
exactly the physics regime (deep moderation in water/iron, epithermal
→ thermal slowing-down tails) where BNCT phantom transport lives.
This directory is a **scaffold**: the data itself is license-restricted
and cannot be committed to this public repository.

## Status: `unexecuted` — data not acquired

Do not claim SINBAD coverage in `VALIDATION.md` or the benchmark
catalogue until a licensed copy exists and the pipeline below has been
run. The catalogue's two-column "status" convention applies: this entry
is `candidate` until executed.

## Why it is gated

SINBAD v2 is distributed under license:

- **NEA Data Bank members** (most countries): request package
  NEA-1937 (Vol. 1) / NEA-1938 (Vol. 2) / NEA-1939 (both) from the
  NEA Data Bank.
- **Canada / United States**: request via RSICC, package DLC-276.
  RSICC can license Vol. 1 to CA/US requesters; Vol. 2 to all eligible
  customers.
- License terms prohibit redistribution — SINBAD content must not be
  committed, vendored, or mirrored in this repository. Add local paths
  to `.gitignore` (see below) before dropping any SINBAD payload in.

Index / search tool (public metadata only):
<https://www.oecd-nea.org/science/wprs/shielding/sinbad/> and the RSICC
SINBAD search tool PSR-580.

## Which entries matter for OpenBNCT

The BNCT validation need is *moderation and transmission in hydrogenous
media* plus *deep-penetration attenuation*. The most relevant reactor-
shielding entries:

| SINBAD id | content | why it anchors us |
|---|---|---|
| `asp_h2o` | Winfrith Water Benchmark | fission-plate source, reaction rates vs depth in water — directly exercises our water σ_t/σ_s and TSL treatment |
| `asp_ng` | Winfrith n-γ through water/steel arrays | water+steel interleaved moderation — tests interface behavior our phantom cases don't |
| `asp_fe` | Winfrith Iron (ASPIS) | iron transmission — the classic deep-penetration benchmark; stresses high-energy groups and downscatter tails |
| `eurac_fe` | Ispra Iron (EURACOS) | second independent iron case |
| NIST water-sphere leakage | neutron leakage spectra from water spheres | spectral (not just integral) water anchor |

Fusion/accelerator entries (FNG, CERF, 14 MeV-driven) are out of scope
for BNCT energy ranges but the same ingestion path applies if ever
wanted.

## Intended ingestion pipeline (when licensed)

1. Place the licensed SINBAD tree outside the repo, e.g.
   `~/.openbnct-data/sinbad-v2/`, and add that path (or an
   `OPENBNCT_SINBAD_DIR` indirection) to `.gitignore`.
2. For each selected experiment, transcribe its SINBAD "benchmark
   specification" (geometry, materials, source spectrum, measured
   reaction rates) into:
   - an `openbnct.transport-case` + material assignment + multigroup
     collapse (the same `sn solve`/`dose` path as FiR-1),
   - an `openbnct.measurement-record/0.1.0` for the measured reaction
     rates/depth profiles — provenance `kind: acquired` (it is licensed
     data, not digitized literature), with `derivation_note` naming the
     SINBAD entry and its license. **The record stays local for the
     same license reason** — what may be committed is the comparison
     *verdict* (chi-square, sigma deviations), which contains no SINBAD
     content.
3. Commit only: this README, the transport-case/assignment (our
   encoding of the public benchmark *specification*), and a
   `measurement-comparison` record with the verdict. The comparison
   record references the measurement by content-hash — reviewers
   holding the same license reproduce bit-for-bit.

## What already substitutes (partial)

- The FiR-1 K63 phantom cases validate in-water thermal penetration at
  reactor-epithermal energies with *measured* endpoints.
- The canonical Reed/Azmy/Kobayashi cases validate the sweep on
  published reference solutions.
- What SINBAD uniquely adds: deep-penetration shielding attenuation
  (orders-of-magnitude drops over tens of cm) and a measured water
  benchmark independent of any BNCT facility's dosimetry conventions.

## Honest limitations

- Acquisition requires an institutional license — this is a
  policy/organizational gate, not a technical one, and is excluded from
  the current technical roadmap by the owner's call.
- If a license is never obtained, the correct substitute is the
  public-domain benchmark literature (e.g. published Winfrith values in
  NSE/journal articles digitized as `digitized_literature` records with
  stated digitization uncertainty).

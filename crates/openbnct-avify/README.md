# openbnct-avify

Connector surface for the separately licensed **Avify Dose** engine,
per `docs/R12_SCOPE_REVIEW.md`.

This crate performs claim-free mechanical work only:

- `voxel` — export an OpenBNCT case + material assignment as the
  engine's voxel plan (`<prefix>_arrays.npz` + `<prefix>_meta.json`),
  mapping regions to the engine's five tissue classes;
- `plan` — the engine's plan JSON schema (declared uptake-uncertainty
  set, weights, criteria, normalisation, run budget, beam);
- `run` — bounded child-process orchestration of `avify-dose` with
  kill-and-reap on timeout;
- `certificate` — typed view of the returned `certificate.json`.

Extremal-map construction, dose-envelope arithmetic, applicability
checks, and certificate derivation remain inside the separately licensed
engine and are never reimplemented here (`docs/IP_BOUNDARY.md`).

Research software — empirical two-evaluation envelopes, not certified
clinical bounds. See `DISCLAIMER.md`.

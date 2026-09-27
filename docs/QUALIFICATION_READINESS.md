# Qualification readiness — research record

`qualification-record.json` at the repository root is the machine-readable
claim/evidence matrix (`openbnct.qualification-record/0.1.0`). Every `held`
claim binds at least one catalogue entry and declares an applicability domain;
`planned` and `external_dependency` entries name the external activity they
wait on and can never be reported as held. This document is the human-readable
half: what the research workflow assumes of its users, what records exist, and
what a future qualified program would need. It changes no software behavior and
makes no clinical claim — see `DISCLAIMER.md`.

## Current workflow and user responsibilities

The software computes research artifacts. The user is responsible for:

- supplying beam/geometry/material inputs that describe their problem — no
  built-in facility commissioning exists;
- reading each report's `qualification` string and `assumptions`/`limitations`
  blocks — they are part of the result, not boilerplate;
- checking `bench verify` before citing a catalogue entry's outcome;
- keeping source/derivation records for any value introduced into a joint-UQ
  input, evidence library, or scenario set;
- treating every `planned`/`reconstructed` dose distinction, every
  surrogate-observation note, and every `unresolved` direction in the exports
  as load-bearing semantics.

## Records that exist

- `benchmark-catalogue.json` — 18 entries, hash-bound to frozen artifacts;
  `bench verify` re-hashes and reports broken references and absent metadata.
- `validation/bio-evidence-library.json` — seeded biological-parameter evidence
  with extraction provenance and applicability context.
- `qualification-record.json` — this package's claim/evidence matrix.
- CI gates (`cargo fmt --check`, `cargo clippy --workspace --all-targets --
  -D warnings`, workspace tests) plus the local bounded-execution rules in
  `AGENTS.md`.
- Repository history — the only release/regression record index that exists.

## Records that do not exist

- Per-case data-license declarations on catalogue entries (reported as
  warnings, not defaulted).
- Real delivery-export formats — only the synthetic CSV import contract.
- Per-study joint covariance in the biological evidence library.
- Institutional review, audit trail, or clinical-usability records of any kind.

## Future-program brief (scope questions for the owner)

A qualified program would need, at minimum and in order:

1. Intended-use definition and the participating facility's role; which
   jurisdiction's device/research rules apply must come from current official
   sources — this document deliberately names no classification.
2. A measurement and calibration plan tied to declared facility hardware —
   detector geometry, output logging formats, phantom truth — feeding real
   artifacts into the delivery-history, replay, and boron-inference paths.
3. Software-quality responsibilities: requirements traceability, change
   control over the frozen evidence, review records per release.
4. Human review and error-handling procedures around every result a person
   could act on.
5. Endpoint adjudication and authorized data use for any outcomes work —
   the export contract is a research record only.

Any of these becomes a *new* program record; it does not retroactively qualify
this codebase or its frozen evidence.

## Revision impact on prior evidence

A new algorithm or data revision produces new artifacts with new hashes.
Frozen entries are never re-generated — a new beam model runs a *new* case
directory and the catalogue entry records its own version field. Passing the
old fixtures does not qualify a new beam model, facility, or biological
endpoint: the claim's applicability domain travels with its evidence, and the
record's software/data versions pin what was evaluated.

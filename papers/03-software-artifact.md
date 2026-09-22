# Paper 3 — OpenBNCT software artifact

**Status:** Blocked on packaging, a tagged release, and archival DOI.

**Date opened:** 2026-08-31

## Target venues

- *Journal of Open Source Software* (JOSS) — preferred. Peer reviewed,
  citable DOI, low cost, and it is what users cite when they *use* the tool
  rather than when they cite the method.
- *SoftwareX* — Elsevier equivalent.

A preprint in `physics.med-ph` on arXiv costs nothing and is readable by the
intended audience before any journal decision lands.

## Claim

A short paper describing the software, its scope boundary, and its
qualification limits. It does not argue a scientific result; Paper 2 does that.
Its function is to give the artifact a stable, citable identity.

## Required evidence

- [x] MIT license
- [x] Contribution guidelines
- [x] Automated tests in CI
- [ ] `pip install openbnct` providing both the module and the CLI, so the tool
      is installable without a Rust toolchain
- [ ] Documentation sufficient for a stranger to install, run, and verify a
      case without assistance
- [ ] A tagged release archived with a DOI
- [ ] Enough working functionality to be *useful*, not only well-engineered

## Notes

The last item is the real gate. JOSS reviewers reject early scaffolding
regardless of code quality; the criterion is substantial scholarly effort in a
tool someone can actually use. Until the R2 dose path works end to end, this
paper describes infrastructure without a function.

Submit after Paper 2's evidence exists, not before.

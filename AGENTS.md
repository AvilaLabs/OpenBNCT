# Local development safety

An unfiltered recursive launch of a Rust test executable exhausted this
laptop's memory on 2026-09-10. These rules apply to all agents working in
this checkout:

- Subagents may inspect and edit code; only the coordinating agent runs
  builds, executable tests, benchmarks, or transport jobs. Run one such
  job at a time.
- On this Linux workstation, run those jobs inside an enforced systemd
  cgroup:
  `systemd-run --user --scope -p MemoryMax=6G -p MemorySwapMax=0 -p TasksMax=128 -p CPUQuota=200% -- env CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 RAYON_NUM_THREADS=2 COMMAND ...`
- If cgroup limits cannot be enforced, stop. Do not fall back to an
  unlimited command. Verify limits with a read-only cgroup inspection,
  never by allocating memory until the OOM killer fires.
- Put temporary build artifacts on disk, not the RAM-backed `/tmp` (it
  fills and returns "quota exceeded"). Create `target/preflight-tmp` and
  set `TMPDIR` to its absolute path inside the bounded scope. This also
  applies to tooling that stages through `/tmp` (gh artifact downloads,
  fortran scratch files — NJOY child processes must get a disk-backed
  `TMPDIR`).
- Do not run a Rust test's `current_exe()` without an explicitly
  isolated, reviewed test-child entry point.
- Review process-spawning code before executing it. Require bounded
  waits, child termination/reaping, and regression coverage for
  cancellation races.
- Do not terminate or reconfigure another assistant's processes. This
  workstation often runs multiple agent sessions and long transport jobs
  (e.g., multi-hour `openmc run`); coordinate shared workload limits with
  the user if contention is observed.

These are local workstation protections, not replacements for CI or
scientific qualification. Never claim that an unexecuted check passed.

# Commit policy

Strict no-AI-authorship: commits carry plain messages only — never add
"Generated with", "Co-Authored-By", or any agent/tool attribution
trailers, names, or links. Author and committer stay the repo owner's
identity.

# Project invariants

- Rust crates under `crates/` are authoritative; the Python package and
  the GUI are surfaces over the same implementation, never parallel
  engines (ADR 0015, ADR 0027).
- Every interchange artifact is versioned and SHA-256 content-bound.
  Frozen evidence under `benchmarks/` and `validation/` is immutable —
  do not regenerate or "fix" committed results.
- The workspace `publish = true` is now lifted: all 15 `openbnct-*`
  crates are on crates.io at v0.1.0; the `openbnct` wheel is on PyPI.
- Research software: no clinical qualification, equivalence,
  commissioning, or regulatory claims. See `DISCLAIMER.md`.
- CI gates include `cargo fmt --check` and clippy `-D warnings` — run
  both before pushing (`cargo fmt --all`, `cargo clippy --workspace
  --all-targets -- -D warnings`).

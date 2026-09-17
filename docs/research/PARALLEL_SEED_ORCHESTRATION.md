# Parallel Seed Orchestration Across Machines

**Status:** Demonstrated manually on two machines (2026-09-16); this document
records the procedure so it is repeatable.

**Context:** the NF-BNCT-001 acceptance contract
(`transport/openmc-acceptance-contract.json`) registers three RNG seeds and
requires candidate-reference runs at **at least three** of them before the
chi-square replication-consistency gate can pass. Each run is a multi-hour
fixed-source OpenMC job, so the wall-clock cost of the gate is set by how the
seeds are distributed across available machines — not by anything in the
contract surface, which is unchanged by where a run executes.

## Why this is contract-safe

The acceptance evaluator binds runs by content, not by machine:

- `openbnct openmc generate` emits a **deterministic** deck; the
  `openbnct-input-manifest` records the execution profile hash, every bound
  artifact hash, the seed, and the declared OpenMC version/commit.
- Regenerating the seed-271828182 deck after the fact produced
  **byte-identical** `settings.xml`, `tallies.xml`, `materials.xml`,
  `geometry.xml`, and manifest bindings — the generator is reproducible.
- No absolute paths are embedded in the deck; the nuclear-data library is
  resolved at runtime via `OPENMC_CROSS_SECTIONS`, so a deck produced on one
  host runs truthfully on another as long as the **same OpenMC build**
  (version `0.16.0`, source commit `617d35a5…`) and the **same case-scoped
  cross-section tree** are used. The manifest records version and commit —
  running a different build would falsify provenance.
- `openmc evaluate` accepts repeated `--run` directories; it enforces seed
  registration, seed uniqueness, per-run precision gates, then the
  cross-seed chi-square. Runs from different machines compose.

## What one seed needs on the worker machine

| Payload | Size | Source |
|---|---|---|
| Generated run directory (XMLs, manifest, acceptance contract, weight windows) | ~48 MB | `openbnct openmc generate --execution-profile <seed-profile> --vr <resolved weight-windows artifact> --acceptance <contract> …` |
| Case-scoped HDF5 cross sections (`endfb-viii.1-hdf5/`, all relative paths) | ~26 MB | `…/openmc-endfb81-official-library/selected/` |
| OpenMC binary + non-system shared libraries + bundled glibc loader | ~190 MB | built tree's `bin/openmc`, `lib/libopenmc.so`, plus `ldd`-resolved deps |

Total payload ≈ 260 MB compressed to ~130 MB as a tarball. The worker needs
no toolchain, no repository checkout, and no OpenBNCT install — only the
tarball and a shell.

## Portable-runtime construction

The OpenMC binary's shared-library closure is bundled explicitly, including
`ld-linux`, `libc`, `libstdc++`, and `libgomp`, and launched through the
bundled loader:

```sh
"$BUNDLE/openmc-portable/lib/ld-linux-x86-64.so.2" \
  --library-path "$BUNDLE/openmc-portable/lib" \
  "$BUNDLE/openmc-portable/bin/openmc"
```

This removes any dependency on the worker's glibc or installed packages —
the same mechanism that lets the bundle run inside a stock WSL2 Ubuntu
install. Verify with `… openmc --version`; it must report the version and
commit recorded in the deck manifest.

One pitfall observed on the coordinating machine: a stale `libopenmc.so`
installed under a user virtualenv shadowed the build-tree library via
`ld.so` search order, producing `undefined symbol: run_mode`. Always ship
the build tree's own `libopenmc.so` (the ~57 MB RelWithDebInfo artifact),
not whatever `ldd` happens to resolve on the build host.

## Execution recipe (per worker)

```sh
tar xzf openbnct-seed-bundle.tar.gz
cd openbnct-seed-bundle
nohup ./run.sh run-seed-<SEED> > run.out 2>&1 &
```

`run.sh` sets `OPENMC_CROSS_SECTIONS` to the bundled tree and
`OMP_NUM_THREADS` (default 6; size to the machine). Progress appears in
`<deck>/openmc.stdout.log` as `Simulating batch N` lines; the run writes
`statepoint.<batches>.h5` on completion. Expect ~4 GB peak RSS and roughly a
day on an 8-thread laptop for the 140-batch / 196M-history deck.

On this project's i3-N305 workstation the run is launched as a transient
cgroup instead of bare `nohup`, so a memory excursion kills the job rather
than the machine:

```sh
systemd-run --user --unit=openbnct-seed-<SEED> \
  -p MemoryMax=6G -p MemorySwapMax=0 -p TasksMax=128 -p CPUQuota=600% \
  -- ./run.sh run-seed-<SEED>
```

## Completion notification without inbound connectivity

A worker behind NAT (including WSL2) can still *originate* connections. A
one-line watcher polls for the transport process and issues an HTTP request
to a listener on the coordinating machine when it exits; the listener's
request log is the notification channel:

```sh
nohup bash -c 'while pgrep -f "bin/openmc" >/dev/null 2>&1; do sleep 300; done;
  curl -s -m 10 "http://<coordinator>:8377/SEED-<SEED>-DONE" >/dev/null 2>&1' &
```

Any HTTP 404 still lands in the coordinator's server log — the ping is the
signal, not the response. A `python3 -m http.server` instance on the
coordinator is sufficient.

## Closing the gate

After all seed run directories return to the coordinator (statepoint +
manifest, ~18 MB each):

```sh
openbnct openmc evaluate \
  --run run-seed-20260831 --run run-seed-271828182 --run run-seed-314159265 \
  --output acceptance-3seed.json
```

`enough_seeds` is satisfied, the chi-square replication-consistency entries
populate, and the per-run precision gates are re-evaluated for each seed.
`vr validate` then compares each VR run against the 600M-history analog
reference report as before.

## Not automated (yet)

This was executed by hand: tarball transfer over LAN HTTP, WSL2 on the
worker, watcher-ping notification. A productionized version would add
machine registration, work-queue handout, artifact push-back, and failure
detection — none of which changes the contract surface. Sequential and
parallel scheduling are interchangeable; on a 4-physical-core worker two
concurrent seeds gain little over sequential execution.

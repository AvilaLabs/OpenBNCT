# S_N vs continuous-energy regression references

Three small references generated with OpenMC 0.16.0 (ENDF/B-VIII.1 neutron
library, `c_H_in_H2O` S(alpha,beta) on hydrogen, 293.6 K) that the test
`crates/openbnct-cli/tests/validation_regression.rs` compares the deterministic
multigroup S_N solver against on every `cargo test`. They are regression guards,
not accuracy certificates: windows are set from the agreement measured when the
suite was written plus margin, so factor-level errors and roughly 20% drifts
fail CI.

| Directory | Case | MC histories |
|---|---|---|
| `infinite-medium-brain/` | 8 cm reflective brain cube, uniform volumetric source | 1e6 |
| `column-1d-brain/` | 1x1x25 brain column of 8 mm cells, +z beam on the z=0 face | 2e6 |
| `mini-layered-head/` | 9x9x9 x 24 mm layered head, disk r=7 cm, cone 0.1491 rad | 2e6 |

Every directory holds `generate_reference.py` (run with
`OMP_NUM_THREADS=3 ~/micromamba/envs/openmc016/bin/python generate_reference.py`),
`reference.json`, `sn-case.json` (the matching S_N case, written by the same
script), `README.md` (physics, normalization) and `data-identity.json` (OpenMC
version, library and S(alpha,beta) hashes, particle counts, seeds). `common.py`
builds the `cross_sections.xml` copy deterministically (library neutron entries
with absolute paths plus the `c_H_in_H2O` table) and holds the shared source
spectrum.

Shared conventions:

- Source spectrum: histogram on [1e-5, 0.5, 1e4, 1.69e7] eV with bin
  probabilities [0.0611, 0.9093, 0.0296], uniform per eV inside each bin. OpenMC
  0.16 `Tabular(interpolation="histogram")` takes a density per eV, so each bin
  probability is divided by its width (checked in `common.py`). The S_N side uses
  `--source-weighting uniform_in_bin`.
- Bands on S_N group boundaries: thermal < 0.5 eV, epithermal 0.5 eV to 10 keV,
  fast > 10 keV.
- S_N data: `benchmarks/synthetic/layered-head-phantom/multigroup-data-28g-v5-tsl.json`.
- Normalization is per source neutron and is derived in each case README.
- `OPENBNCT_REG_REUSE_DIR=<dir>` re-reads an existing statepoint instead of
  running OpenMC (post-processing only).

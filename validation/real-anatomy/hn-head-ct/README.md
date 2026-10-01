# Real head CT through the OpenBNCT golden path

A public head-and-neck radiotherapy planning CT, taken through
`openbnct project init --ct-nifti ...` and `openbnct project run` at 4 mm, then
re-computed with continuous-energy OpenMC by `openbnct project verify`.
Research software; nothing here is a clinical result, a treatment plan or a
validation for any use on patients.

**The target is synthetic.** `RESEARCH_TARGET` is a sphere placed by
`prepare.py` to exercise the workflow. It is not a tumor, not a segmentation of
any lesion, and represents no patient finding. The boron concentrations and
tissue:blood ratios are illustrative values, not patient or study data.

## Data and attribution

Source: `google-deepmind/tcia-ct-scan-dataset`, licensed CC BY 4.0 (data and
code), commit `2edb50947e50ef26e60b72dec89d36d71efe5549`. Used: the test-set scan
`nrrds/test/oncologist/0522c0226/CT_IMAGE.nrrd` and its 21 organ-at-risk
segmentations (oncologist-arbitrated set). The raw CT and segmentations are not
redistributed here; `prepare.py` fetches them from commit-pinned URLs and checks
the sha256 of every file against `fetch-pins.json`.

Please cite, as the dataset requests:

> S. Nikolov, S. Blackwell, R. Mendes, J. De Fauw, C. Meyer, C. Hughes, H. Askham,
> B. Romera-Paredes, A. Karthikesalingam, C. Chu, D. Carnell, C. Boon, D. D'Souza,
> S. A. Moinuddin, K. Sullivan, DeepMind Radiographer Consortium, H. Montgomery,
> G. Rees, R. Sharma, M. Suleyman, T. Back, J. R. Ledsam, O. Ronneberger, "Deep
> learning to achieve clinically applicable segmentation of head and neck anatomy
> for radiotherapy," 2018. arXiv:1809.04430.

The scans are depersonalised CT planning scans originally from The Cancer Imaging
Archive (Clark et al. 2013): TCGA-HNSC (Zuley et al. 2016) and Head-Neck Cetuximab
(Bosch et al. 2015); segmentations follow the Brouwer et al. (2015) atlas. See the
dataset README for the full references. The derived files committed here
(label names, derived structure masks, case geometry) are derivatives of that data
and are shared under the same CC BY 4.0 terms; changes were made (cropping,
resampling to 4 mm, derived structures).

**Why this scan.** 0522c0226 has 162 axial slices (0.98 x 0.98 x 2.5 mm), the whole
cranium and brain with 11 slices (27 mm) of air above the scalp and the neck below,
and the brain labelled. Scans whose brain reaches the top slice (several in the
set) were rejected because they truncate the scalp.

## Processing chain

1. `prepare.py` (Python: pynrrd, nibabel, numpy, scipy) fetches and verifies the
   files, converts NRRD to NIfTI and derives the extra structures:
   - Orientation: the NRRD is already axis-aligned left-posterior-superior
     (diagonal `space directions`), so nothing is resampled. The code is general
     (RAS is converted to LPS, array axes are permuted to x, y, z and flipped to
     increasing coordinate, oblique grids stop with an error). The NIfTI affine
     (RAS+) is `diag(-sx, -sy, sz)` with translation `(-ox, -oy, oz)`, which
     OpenBNCT reads back as an identity-direction LPS grid.
   - BODY: HU > -400, largest 6-connected component, holes filled (per axial
     slice, then in 3-D). The couch is a separate component and is excluded.
   - SKIN: the outer 5 mm shell of BODY (distance to the outside of BODY).
   - RESEARCH_TARGET (synthetic): a 25 mm radius sphere centred 35 mm inside the
     scalp surface along the beam line, intersected with Brain (62.3 of 65.4 cm3
     kept). The line passes through the brain centroid's (y, z). Centre
     (-36.5, 402.8, -261.0) mm LPS.
   - Crop: to 20 mm below the mandible upward and the BODY bounding box plus 15 mm
     of air in x, y (218 x 272 x 91 voxels at native resolution). The full field of
     view including the torso (1.6 M voxels at 4 mm) exceeded the 7 GiB memory cap
     the benchmark runs under. Structures are derived before the crop; the lungs,
     empty after it, are dropped.
   - Output: `hu.nii.gz`, an int32 bitmask `labels.nii.gz` (one bit per structure, so
     structures may overlap) and `label-names.json`.
2. `openbnct project init --ct-nifti data/hu.nii.gz --labels-nifti data/labels.nii.gz
   --label-names data/label-names.json --output project --target RESEARCH_TARGET
   --spacing-mm 4 --id openbnct.hn-head-ct.v1`
3. `project/project.toml` edits: S4 (`order = 4`; the init default is 8); blood 25
   ug/g; tissue:blood ratios RESEARCH_TARGET 3.5, SKIN 1.5, everything else 1.0
   (illustrative); `[report] structures` excludes the two cochleae (0 voxels at
   4 mm). Beam `fir1-k63-ineel`, approach `+x` (the beam travels toward the
   patient's left and enters through the right lateral scalp, the nearest scalp to
   the target).
4. `RAYON_NUM_THREADS=6 openbnct project run project`, then
   `OMP_NUM_THREADS=3 openbnct project verify project --particles 6e6 --threads 3`.

Case grid after import: 54 x 67 x 57 = 206,226 voxels at 4 mm. Transport: 28-group
S4, P1 scattering (from the data), CMFD, Anderson depth 3.

To regenerate everything: `python prepare.py` (writes `data/`), then steps 2 to 4.

## Runs

**Final run (headline).** Transported photons (`photon_transport = true`, the
project default), OpenBNCT built from origin/main `b46ffb2` (S_N mass-fraction
blending of mixed voxels, photon solve with CMFD): capture and inelastic photons
solved with `sn photon-solve`. Import, calibrate and beam-bind outputs were
reused (`project run --from transport`); `project verify` re-ran OpenMC. Results in
`results/transported-photon/`. The committed `project/project.toml` is this
configuration.

**Comparison run (earlier build, not the headline).** Local capture-gamma
deposition (`photon_transport = false`): capture-gamma energy is deposited where it
is born, which is known to over-predict the photon dose in a finite head. It was
run on the build before the mass-fraction change, so its neutron-component ratios
differ slightly from the final run (for example SKIN nitrogen 1.56 there). Results
in `results/local-photon/`, with the `project.toml` it used.

## Timing and memory (final run, build `b46ffb2`)

Shared 8-thread laptop with other jobs running, so wall times are not clean
benchmarks. `RAYON_NUM_THREADS=6` for the run, `OMP_NUM_THREADS=3` for OpenMC.

| step | final run | earlier local-deposition run |
|---|---|---|
| import (`import ct-nifti`) | 2.7 s (first run, reused) | 2.7 s |
| calibrate | 0.4 s (reused) | 0.4 s |
| beam bind | <0.1 s (reused) | <0.1 s |
| transport (neutron + photon) | 925.8 s: neutron S4 outer loop 300 s (18 outers, residual 5.6e-7), two photon solves 12 outers each (loops 85 s and 80 s), remainder is setup | 1538.5 s (neutron only; loop 728 s) |
| boron | 0.3 s | 0.2 s |
| metrics (+ DVH) | 27.4 s | 29.4 s |
| `project run --from transport` wall | 15:54, peak 2.28 GB | 26:08, peak 2.30 GB |
| `project verify`: 6e6 histories, 10 batches, 3 threads | 1268 s, peak 0.35 GB | 1687 s |

Grid: 54 x 67 x 57 = 206,226 voxels at 4 mm; 28 neutron groups, 16 photon groups.

## Dose (Gy per source particle, structure means; final run, local-photon column from the earlier run)

| structure | boron | nitrogen | hydrogen | photon, local | photon, transported |
|---|---|---|---|---|---|
| RESEARCH_TARGET (synthetic) | 9.29e-14 | 9.4e-16 | 7.1e-16 | 1.09e-13 | 1.39e-14 |
| Brain | 1.27e-14 | 4.3e-16 | 3.4e-16 | 3.88e-14 | 6.75e-15 |
| SKIN | 4.76e-15 | 1.56e-16 | 3.2e-16 | 1.31e-14 | 2.84e-15 |

Boron dose is scaled by blood 25 ug/g and the illustrative ratios above.

## Independent check (OpenMC, continuous energy, ENDF/B-VIII.1)

S_N / MC ratio of structure-mean dose per component, 6e6 histories, final run
(build `b46ffb2`). **Statistics caveat: 6e6 histories leave the small structures
unresolved (median voxel 1-sigma about 100 %); the structure-mean results for the
large structures (Brain, BODY, SKIN, Mandible, the parotids, RESEARCH_TARGET) are
the meaningful result.** The photon
column is the point of the two runs: local deposition overshoots by a factor of
3 to 8 in the larger structures (6.0 in the brain, 8.1 in the target); transported photons
agree to within the MC noise. The MC photon tally is noisy (median relative 1-sigma
of individual voxels near 100 %), so read the photon and total columns as
structure means only.

Final run (transported photons):

| structure | voxels | boron | nitrogen | hydrogen | photon | total | status |
|---|---|---|---|---|---|---|---|
| Brain | 19291 | 0.951 | 0.958 | 0.930 | 1.050 | 0.982 | agrees |
| Brainstem | 357 | 0.981 | 0.986 | 0.961 | 1.099 | 1.036 | agrees |
| Lacrimal-Lt | 7 | 1.146 | 1.148 | 1.218 | 0.970 | 0.983 | agrees |
| Lacrimal-Rt | 6 | 0.953 | 0.957 | 1.072 | 0.902 | 0.920 | unresolved |
| Lens-Lt | 1 | 1.047 | 1.048 | 1.228 | 0.836 | 0.895 | unresolved |
| Lens-Rt | 2 | 1.019 | 1.020 | 1.186 | 1.352 | 1.169 | unresolved |
| Mandible | 1222 | 0.996 | 0.997 | 0.974 | 1.022 | 1.008 | agrees |
| Optic-Nerve-Lt | 5 | 1.022 | 1.021 | 1.012 | 0.789 | 0.868 | unresolved |
| Optic-Nerve-Rt | 5 | 0.957 | 0.971 | 0.903 | 0.973 | 0.946 | unresolved |
| Orbit-Lt | 116 | 1.038 | 1.043 | 1.005 | 1.139 | 1.096 | unresolved |
| Orbit-Rt | 116 | 0.973 | 0.986 | 1.049 | 1.045 | 1.006 | agrees |
| Parotid-Lt | 520 | 1.150 | 1.120 | 1.231 | 0.993 | 1.020 | agrees |
| Parotid-Rt | 579 | 0.984 | 0.967 | 1.064 | 1.034 | 1.003 | agrees |
| Spinal-Canal | 277 | 1.059 | 1.059 | 1.226 | 1.186 | 1.134 | unresolved |
| Spinal-Cord | 73 | 1.077 | 1.086 | 1.246 | 1.105 | 1.097 | unresolved |
| Submandibular-Lt | 115 | 1.077 | 1.067 | 0.837 | 1.023 | 1.029 | agrees |
| Submandibular-Rt | 95 | 1.113 | 1.111 | 0.857 | 0.865 | 0.930 | unresolved |
| BODY | 66852 | 0.976 | 0.982 | 0.980 | 1.052 | 1.003 | unresolved |
| SKIN | 10415 | 1.064 | 1.037 | 1.067 | 1.042 | 1.054 | unresolved |
| **RESEARCH_TARGET** (synthetic) | 959 | 0.941 | 0.950 | 0.931 | 1.032 | 0.952 | agrees |

**Verdict (as configured: total-dose ratio within +-5 %, gamma 3 %/3 mm pass
>= 95 % where evaluated): INCONCLUSIVE.** No structure with resolved MC statistics
disagrees, but 11 structures (the small ones, plus BODY and SKIN) could not be
tested at +-5 % with 6e6 histories. "agrees" means the structure-mean total-dose
ratio is within 5 % of 1; BODY's gamma pass rate is 0.0 % on a single evaluated
voxel, which is the MC photon noise rather than a disagreement. Whole-volume gamma
(3 %/3 mm, global, S_N against MC): boron 92.5 % (11674 voxels), nitrogen 66.8 %,
hydrogen 44.4 %, photon and total 0 % on 168 and 172 voxels.

Neutron components in the large structures (final run): boron 0.95 to 1.06
(Brain 0.951, target 0.941, SKIN 1.064, BODY 0.976), nitrogen 0.95 to 1.04 (SKIN
1.037, BODY 0.982), hydrogen 0.93 to 1.07 (Brain 0.930, SKIN 1.067, BODY 0.980).
The mass-fraction blending of mixed voxels moved SKIN nitrogen from 1.56 to 1.04
and BODY from 1.14 to 0.98 relative to the earlier build. The hydrogen ratios near
1.2 in the spine and some small structures rest on a few voxels and were not
investigated. SKIN's total ratio is 1.054, just outside the 5 % band, with a
36 % (2 sigma) MC uncertainty, so it is unresolved rather than a disagreement.

Earlier local capture-gamma run (previous build, same MC statistics): photon ratio
6.03 in the brain, 8.10 in the target and 4.8 to 6.3 in most other large
structures (total ratio 2.5 in the brain, 1.79 in the target); verdict
INCONCLUSIVE, with every structure unresolved or outside the band.

## What is committed and what is regenerated

Committed: `prepare.py`, `fetch-pins.json`, `data/label-names.json`,
`data/derived.json` (target placement, volumes, hashes), `project/project.toml`,
`project/inputs/` (the built-in artifacts the project copies, about 3 MB),
`derived/` (4 mm scaffold and beam-bound case JSON, import record, mask index, the
RESEARCH_TARGET mask, calibration report), and `results/` for both runs
(`report.md`, `report.json`, `verification.json`, `comparison.json`,
`run-manifest.json`, `project.toml`).

Regenerated by `prepare.py` / `project run` (too large to commit; over ~5 MB in total):
the raw CT and segmentations (`cache/`), `data/hu.nii.gz` and `data/labels.nii.gz`,
the material assignment (28 MB), the other 21 masks (2 MB each), `out/`, and the
per-voxel gamma file.

## Limitations

- The target is synthetic and the ratios are illustrative.
- 4 mm voxels: the cochleae vanish, the lenses and optic nerves are 1 to 5 voxels.
- The CT is cropped to head and neck; the torso is not modelled.
- Statistics: 6e6 histories could not resolve the small structures. More histories
  would be needed for a conclusive verdict.
- One scan; no claim beyond this case.

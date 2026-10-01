# Review a real-head case

This page is for researchers asked to look at OpenBNCT. It gives one case you can run end to end, what the case currently shows, and the comparisons where an independent result would help most.

## What OpenBNCT does

- Takes a CT and its structures to BNCT component doses. Inputs are DICOM CT with RTSTRUCT or SEG, NIfTI, NRRD or MetaImage. Outputs are boron, nitrogen, hydrogen/fast-neutron and photon dose, DVHs and a report. A single project directory drives it from the CLI (`openbnct project init` / `run`) or the desktop Project workspace.
- Solves 28-group neutron and 16-group photon transport deterministically (S_N with thermal S(α,β), P1 scattering, CMFD acceleration) on the CT grid.
- Re-runs the same project in continuous-energy OpenMC with one command (`openbnct project verify`) and reports per-structure S_N/Monte Carlo ratios and gamma pass rates. Optional CADIS weight windows resolve small structures faster.
- Is open source (MIT), installable with `cargo install openbnct-cli` or `pip install openbnct`, with desktop builds on [GitHub Releases](https://github.com/AvilaLabs/OpenBNCT/releases/latest).

## The case: a public head CT

The [head-and-neck CT benchmark](https://github.com/AvilaLabs/OpenBNCT/blob/main/validation/real-anatomy/hn-head-ct/README.md) uses a CC BY 4.0 planning CT (TCIA / DeepMind, scan 0522c0226) with 21 contoured organs at risk. The setup:

- **Grid:** resampled to 4 mm, 54 × 67 × 57 voxels.
- **Beam:** the FiR 1 K63 epithermal beam, entering laterally.
- **Target:** a synthetic research target in the brain. It is not a lesion.
- **Boron:** illustrative concentrations (blood 25 µg/g; target 3.5:1, skin 1.5:1).

To reproduce it from source (the scan is fetched from commit-pinned URLs and checksum-verified; `prepare.py` needs `pynrrd`, `nibabel`, `numpy` and `scipy`):

```bash
git clone https://github.com/AvilaLabs/OpenBNCT.git && cd OpenBNCT
cargo build --release -p openbnct-cli
cp -r validation/real-anatomy/hn-head-ct review-head && cd review-head
python3 prepare.py
# the recorded run predates automatic body cropping; keep the full field of view
sed -i 's/^spacing_mm = 4.0$/spacing_mm = 4.0\ncrop = "none"/' project/project.toml
../target/release/openbnct project run project
../target/release/openbnct project verify project --particles 6e6
```

`project verify` needs OpenMC 0.16 and an ENDF/B-VIII.1 HDF5 library that includes the thermal-scattering files.

## What it currently shows

- **Agreement with Monte Carlo:** structure-mean total dose, S_N/MC, is 0.982 for brain, 1.003 for BODY and 0.952 for the synthetic target. Ratios are against 6 million OpenMC histories with matched thermal scattering and transported photons. These are regional means. The recorded verdict is **INCONCLUSIVE**: eleven small structures were unresolved at that Monte Carlo statistics, and voxel-level photon dose was noisy.
- **Runtime:** the deterministic run takes about 4 minutes on an 8-thread laptop (Intel i3-N305) with OpenBNCT 0.3.0. The recorded run took 16 minutes on an earlier build.
- **Measured phantom:** the FiR 1 K63 water-cylinder thermal-fluence depth profile was [rerun on 0.3.0](https://github.com/AvilaLabs/OpenBNCT/blob/main/validation/fir1-k63-cylindrical-rerun-2026-10/README.md). The absolute scale is right at 3–5 cm; the earlier 1.6–2.3× underprediction is gone. The computed profile is flatter than measured: about 40% low at 1 cm and about 1.5× high at 8–12 cm. It does not pass, and attributing that shape difference is where measured data with original uncertainties would help most.

Details and older configurations are on [Benchmarks and code comparisons](benchmarks.md).

## Where an independent check would help most

- **An independent transport code on this case** (MCNP, PHITS, GATE/Geant4). The OpenMC comparison shares nuclear data and the source definition with OpenBNCT's own pipeline. A second code removes that dependence.
- **Measured phantom data** with original uncertainties, source description and normalization. Digitized published profiles limit how sharply a residual can be attributed.
- **DICOM semantics** in a second application (for example SlicerRT): do structures, the dose grid and component units land where intended?
- **Component-dose conventions:** whether OpenBNCT's boron, nitrogen, hydrogen and photon definitions are comparable to what your code scores.

Open an issue on [GitHub](https://github.com/AvilaLabs/OpenBNCT/issues) or reply to the message that brought you here. Disagreements are as useful as agreements. Please report a mismatch with its inputs, and it will be recorded alongside the case.

OpenBNCT is research software. Nothing here is a clinical result, a treatment plan or a qualification for use on patients; see [Research scope and qualification](scope.md).

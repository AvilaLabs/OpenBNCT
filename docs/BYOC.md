# Bring your own case

OpenBNCT's transport inputs are plain versioned JSON — this is the
shortest path from data you already have to a running problem. All three
on-ramps below end at the same place: a `transport-case` +
`material-assignment` pair you can hand to `sn solve`, `openmc
generate`, `export mcnp`, or `export phits`.

## From a segmented phantom (NIfTI labelmap)

If you have a voxel phantom as an integer-labeled NIfTI — exported from
3D Slicer, ITK-SNAP, or one `nibabel` line — that *is* a material
assignment waiting to happen. You need two files:

**`phantom.nii`** — integer labels per voxel (`0` = background/base
material). `.nii`, `.nii.gz`, NRRD (`.nrrd`/`.nhdr`) or MetaImage
(`.mha`/`.mhd`); see "Supported input formats" in `docs/USAGE.md`.

**`materials.json`** — a label → material-definition map:

```json
{
  "0": {"schema_version": "openbnct.material-definition/0.1.0",
        "id": "mylab.air.v1", "density_g_cm3": 0.0012,
        "temperature_k": 293.6,
        "nuclides": [{"name": "N14", "mass_fraction": 0.755},
                     {"name": "O16", "mass_fraction": 0.232},
                     {"name": "H1",  "mass_fraction": 0.013}],
        "neutron_thermal_treatment": "free_gas"},
  "1": {"schema_version": "openbnct.material-definition/0.1.0",
        "id": "mylab.water.v1", "density_g_cm3": 1.0, "...": "..."}
}
```

Then:

```text
openbnct import labelmap --nifti phantom.nii --materials materials.json \
    --case-output case.json --case-id mylab.phantom.v1 \
    --output assignment.json
```

Emits `assignment.json` (one `voxel_set` region per label,
provenance-bound to the labelmap's sha256) plus a scaffold `case.json`
carrying the labelmap's grid and a **placeholder** mono-thermal disk
source — replace it with your real beam via `beam build` + `beam bind`
below.

Already have a transport-case JSON instead? `--case existing.json` binds
to it (the labelmap grid must match the case grid exactly; the base
material comes from the case).

## From DICOM

A CT series (with an optional RT Structure Set) is the first-class path.
`dicom import-ct` resamples it onto a transport grid, writes the HU volume on
that grid, a scaffold case, and (with `--masks-dir`) one mask per ROI:

```text
openbnct dicom import-ct --series /path/to/dicom-dir \
  --spacing-mm 5 --case-id mylab.patient.v1 \
  --base-material void.json \
  --case-output case.json --hu-output hu.nii \
  --masks-dir masks

openbnct dicom calibrate --calibration hu-calibration.json \
  --hu-nifti hu.nii --case case.json --output assignment.json
```

- `--series DIR` is searched recursively; use `--slices f1 f2 ...` and
  `--rtstruct FILE` to name files explicitly. Exactly one CT series and at
  most one structure object are accepted; anything else is refused. The
  structure object is an RT Structure Set **or a DICOM Segmentation (SEG)**
  whose frames lie on the CT lattice (binary, or fractional thresholded at
  0.5); each segment becomes one ROI mask.
- `--spacing-mm` is the transport voxel size: one value (isotropic) or
  `x,y,z`. Without it the native CT spacing is kept.
- HU is **volume-averaged**, not point-sampled: each transport voxel gets the
  mean HU of the CT voxels it overlaps, weighted by overlap volume (parts of
  a transport voxel outside the CT are left out of the mean). The grid uses
  the CT's own patient frame and direction cosines, so nothing is reoriented,
  and it covers the CT (the last voxel per axis may extend past it).
- ROI masks are rasterized on the CT grid by the RTSTRUCT importer (a CT
  voxel is inside when its center lies inside the contour polygon), then a
  transport voxel is in the ROI when at least 50 % of its volume is covered
  by such CT voxels. `masks/index.json` lists each mask with its sha256.
- `--base-material` is the `openbnct.material-definition` used as the case's
  background material (for example `void.json`).
- The scaffold source is a **placeholder** (as in `import labelmap`); replace
  it with your real beam via `beam build` + `beam bind` below.
- Every input file and every output is hash-bound in
  `case.import-record.json` (next to `--case-output`). Existing outputs are
  never overwritten.

Have a clinical plan's RT Dose to compare against? `openbnct import rtdose
--file dose.dcm --case case.json --output dose-bundle.json` brings it onto the
case grid (trilinear resample when the grids differ; `DoseUnits` must be GY)
with a hash-bound import record.

Have the CT as NRRD or MetaImage instead of DICOM? Convert it once with
`openbnct import volume --input ct.nrrd --output ct.nii`; every command that
takes a NIfTI volume also accepts `.nrrd`/`.nhdr`/`.mha`/`.mhd` directly.

HU values become per-voxel materials via the calibration curve. PET/SUV uptake
layers on top via `dicom import-pet`. See `docs/USAGE.md` for the full chain.

## From a measured or digitized spectrum

Facility spectra usually live as a table — digitized from a paper or
written out by a measurement. Bin it into `e_low_ev,e_high_ev,weight`
rows (edges must tile without gaps):

```text
1e-5,0.5,0.0611
0.5,1e4,0.9093
1e4,1.7e7,0.0296
```

Then declare the port geometry and emit a beam-description:

```text
openbnct beam build --id mylab.beam.thermal.v1 \
    --name "Thermal column" --facility "My Facility" \
    --spectrum-csv spectrum.csv --radius-cm 5.0 \
    --divergence-deg 8.6 \
    --cite "Lab internal;Column characterization;Internal memo;2024" \
    --output beam.json
openbnct beam info --beam beam.json        # validates + summarizes
openbnct beam bind --beam beam.json --case case.json --output bound.json
```

The citation is required — the contract is provenance-bound, and an
internal characterization memo is a legitimate citation. Port centers
are in *world* coordinates, so `--center-uv-cm` is the beam axis
position on the entry face, not a face-local offset.

## Then solve

```text
openbnct sn collapse ...                   # build multigroup data for your nuclides
openbnct sn solve --case bound.json --data multigroup.json \
    --assignment assignment.json --dose dose.json --output flux.json
```

The solver refuses honestly if the multigroup data lacks one of your
materials — that's the data-acquisition step, not a format problem.
`docs/USAGE.md` covers `sn collapse` against ENDF-B/VIII.1 sources.

## Then evaluate boron

`sn collapse` also records the tissue-independent ¹⁰B response per µg/g,
so the ¹⁰B concentration can be changed after transport without
rebuilding materials (trace-¹⁰B approximation: the applied boron does
not perturb the flux):

```text
openbnct sn solve --case bound.json --data multigroup.json \
    --assignment assignment.json --dose dose.json \
    --boron-unit-output unit-dose.json --output flux.json
openbnct boron dose --physical-bundle dose.json --unit-dose unit-dose.json \
    --blood-ug-g 25 --ratio tumor=3.5 --mask tumor=tumor-mask.json \
    --output dose-25ug.json
```

Multigroup data collapsed before this feature lacks the unit vector;
re-run `sn collapse`. See "Post-hoc boron" in `docs/USAGE.md`.

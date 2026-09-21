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
material). `.nii` or `.nii.gz`.

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

A CT series is the first-class path:

```text
openbnct dicom import-ct --series /path/to/ct --output case.json
openbnct dicom calibrate --case case.json --output assignment.json
```

RTSTRUCT masks become named regions; HU values become per-voxel
materials via the calibration curve. PET/SUV uptake layers on top via
`dicom import-pet`. See `docs/USAGE.md` for the full chain.

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

# Imaging, materials and beams

A transport study needs a grid, material assignment and source. Images and a beam description provide inputs to those declarations; importing a file does not establish their physical suitability.

## CT and structures

The project workflow accepts a DICOM CT series with supported structures. The lower-level `dicom import-ct` path creates a transport grid, HU volume, scaffold case and ROI masks. The import accepts one CT series and at most one RT Structure Set or supported CT-lattice SEG object.

Transport voxels receive volume-averaged HU values from overlapping CT voxels. The grid retains the patient frame and direction cosines. Structure masks are resampled under the declared occupancy rule. Inspect spacing, orientation and structures before transport.

HU-to-material calibration assigns density and composition. The bundled generic calibration is a demonstration input; bind a suitable declared calibration for a research study.

Current source also accepts scalar HU NIfTI with `import ct-nifti` or `project init --ct-nifti`. An integer labelmap, label-name map and explicit target can supply structures. These paths preserve the image geometry and write import evidence. Use the [CT benchmark](https://github.com/AvilaLabs/OpenBNCT/tree/main/validation/real-anatomy/hn-head-ct) for a documented public-data example; its target is synthetic.

## Segmented phantoms

An integer NIfTI labelmap plus a label-to-material JSON table can create the same transport contracts:

```bash
openbnct import labelmap --nifti phantom.nii --materials materials.json \
  --case-output case.json --case-id mylab.phantom.v1 \
  --output assignment.json
```

The scaffold beam is a placeholder. Replace it with the intended source before solving. Supported volume paths also include NRRD and MetaImage; consult the reference for their restrictions.

## Beam definition

A beam description declares spectrum bins, weights, port size, direction and source geometry. `beam build` accepts measured/digitized spectrum tables; `beam bind` applies a beam to a case. Bin edges must tile consistently. Declare the within-bin spectrum weighting used by the solver.

For MR/PET workflows, register and resample the image onto the case frame before using values for masks or uptake. A registered PET field still needs a declared conversion model.

Complete input schemas and commands: [bring your own case](https://github.com/AvilaLabs/OpenBNCT/blob/main/docs/BYOC.md) and [usage reference](https://github.com/AvilaLabs/OpenBNCT/blob/main/docs/USAGE.md).

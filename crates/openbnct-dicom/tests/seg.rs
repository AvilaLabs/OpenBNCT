// SPDX-License-Identifier: MIT

//! DICOM SEG import against the synthetic NF-BNCT-001 CT.

use std::fs;
use std::path::PathBuf;

use openbnct_dicom::synthetic::{
    CT_SERIES_INSTANCE_UID, FRAME_OF_REFERENCE_UID, STUDY_INSTANCE_UID, SyntheticSegSpec,
    SyntheticSegment, generate_nf_bnct_001, write_seg,
};
use openbnct_dicom::{
    CtVolume, SegImportOptions, SegmentationType, StructureSource, import_ct_contours_from_paths,
    import_ct_series, import_seg, import_study_from_files,
};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    ct_files: Vec<PathBuf>,
    rtstruct: PathBuf,
    ct: CtVolume,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("case");
    let generated = generate_nf_bnct_001(&root).unwrap();
    let ct = import_ct_series(&generated.ct_files).unwrap();
    Fixture {
        _dir: dir,
        root,
        rtstruct: generated.rtstruct_file.clone(),
        ct_files: generated.ct_files,
        ct,
    }
}

fn box_values(ct: &CtVolume, lo: [usize; 3], hi: [usize; 3], value: f64) -> Vec<f64> {
    let [nx, ny, nz] = ct.geometry.shape.map(|v| v as usize);
    let mut v = vec![0.0; nx * ny * nz];
    for k in lo[2]..hi[2] {
        for j in lo[1]..hi[1] {
            for i in lo[0]..hi[0] {
                v[k * nx * ny + j * nx + i] = value;
            }
        }
    }
    v
}

fn spec(ct: &CtVolume, segments: Vec<SyntheticSegment>, max: Option<u16>) -> SyntheticSegSpec {
    SyntheticSegSpec {
        geometry: ct.geometry.clone(),
        frame_of_reference_uid: FRAME_OF_REFERENCE_UID.into(),
        study_instance_uid: STUDY_INSTANCE_UID.into(),
        referenced_series_uid: Some(CT_SERIES_INSTANCE_UID.into()),
        max_fractional: max,
        segments,
        shared_orientation: true,
    }
}

fn segment(number: i32, label: &str, values: Vec<f64>) -> SyntheticSegment {
    SyntheticSegment {
        number,
        label: label.into(),
        values,
    }
}

#[test]
fn binary_multi_segment_maps_onto_ct_grid() {
    let f = fixture();
    let a = box_values(&f.ct, [10, 12, 5], [20, 22, 15], 1.0);
    let b = box_values(&f.ct, [25, 3, 30], [28, 9, 33], 1.0);
    let path = f.root.join("seg.dcm");
    write_seg(
        &path,
        &spec(
            &f.ct,
            vec![
                segment(1, "TUMOR", a.clone()),
                segment(3, "BRAIN_STEM", b.clone()),
            ],
            None,
        ),
    )
    .unwrap();
    let import = import_seg(&path, Some(&f.ct), &SegImportOptions::default()).unwrap();
    assert_eq!(import.segmentation_type, SegmentationType::Binary);
    assert_eq!(import.geometry, f.ct.geometry);
    assert_eq!(import.fractional_threshold, None);
    assert_eq!(import.segments.len(), 2);
    assert_eq!(import.segments[0].label, "TUMOR");
    assert_eq!(import.segments[0].frame_count, 10);
    let tumor = import.structures.roi("TUMOR").unwrap();
    assert_eq!(tumor.number, 1);
    assert_eq!(tumor.voxel_count(), 10 * 10 * 10);
    assert_eq!(tumor.voxels, a.iter().map(|v| *v > 0.0).collect::<Vec<_>>());
    let stem = import.structures.roi("BRAIN_STEM").unwrap();
    assert_eq!(stem.number, 3);
    assert_eq!(stem.voxel_count(), 3 * 6 * 3);
    assert_eq!(stem.voxels, b.iter().map(|v| *v > 0.0).collect::<Vec<_>>());
    // Same centroid semantics as RTSTRUCT masks.
    let c = tumor.centroid_lps_mm(&f.ct).unwrap();
    assert!((c[2] - (-97.5 + 5.0 * 9.5)).abs() < 1e-9, "{c:?}");
}

#[test]
fn per_frame_orientation_and_own_grid_without_ct() {
    let f = fixture();
    let a = box_values(&f.ct, [10, 12, 5], [20, 22, 15], 1.0);
    let path = f.root.join("seg-own.dcm");
    let mut s = spec(&f.ct, vec![segment(2, "TARGET", a.clone())], None);
    s.shared_orientation = false;
    write_seg(&path, &s).unwrap();
    let import = import_seg(&path, None, &SegImportOptions::default()).unwrap();
    // Own grid spans only slices 5..=14 of the original lattice.
    assert_eq!(import.geometry.shape, [40, 40, 10]);
    assert_eq!(import.geometry.spacing_mm, [5.0, 5.0, 5.0]);
    assert_eq!(import.geometry.origin_mm, [-97.5, -97.5, -97.5 + 25.0]);
    assert_eq!(
        import.geometry.direction,
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
    );
    let roi = import.structures.roi("TARGET").unwrap();
    assert_eq!(roi.voxel_count(), 1000);
    assert_eq!(roi.voxels.len(), 40 * 40 * 10);
    // Voxel (10, 12, 0) of the own grid is original (10, 12, 5).
    assert!(roi.voxels[12 * 40 + 10]);
    assert!(!roi.voxels[12 * 40 + 9]);
}

#[test]
fn fractional_uses_the_declared_threshold() {
    let f = fixture();
    let strong = box_values(&f.ct, [10, 10, 10], [14, 14, 14], 0.9);
    let mut weak = box_values(&f.ct, [20, 20, 20], [24, 24, 24], 0.4);
    weak[0] = 1.0; // one certain voxel at the origin corner (slice 0)
    let path = f.root.join("seg-frac.dcm");
    write_seg(
        &path,
        &spec(
            &f.ct,
            vec![segment(1, "STRONG", strong), segment(2, "WEAK", weak)],
            Some(255),
        ),
    )
    .unwrap();
    let default = import_seg(&path, Some(&f.ct), &SegImportOptions::default()).unwrap();
    assert_eq!(default.segmentation_type, SegmentationType::Fractional);
    assert_eq!(default.fractional_threshold, Some(0.5));
    assert_eq!(default.structures.roi("STRONG").unwrap().voxel_count(), 64);
    assert_eq!(default.structures.roi("WEAK").unwrap().voxel_count(), 1);
    let low = import_seg(
        &path,
        Some(&f.ct),
        &SegImportOptions {
            fractional_threshold: 0.3,
        },
    )
    .unwrap();
    assert_eq!(low.structures.roi("WEAK").unwrap().voxel_count(), 64 + 1);
    let bad = import_seg(
        &path,
        Some(&f.ct),
        &SegImportOptions {
            fractional_threshold: 1.5,
        },
    );
    assert!(bad.is_err());
}

#[test]
fn refuses_wrong_frame_off_lattice_and_wrong_series() {
    let f = fixture();
    let a = box_values(&f.ct, [10, 12, 5], [20, 22, 15], 1.0);
    let path = f.root.join("bad.dcm");
    let opts = SegImportOptions::default();

    let mut wrong_frame = spec(&f.ct, vec![segment(1, "A", a.clone())], None);
    wrong_frame.frame_of_reference_uid = "2.25.1".into();
    write_seg(&path, &wrong_frame).unwrap();
    let err = import_seg(&path, Some(&f.ct), &opts)
        .unwrap_err()
        .to_string();
    assert!(err.contains("Frame of Reference"), "{err}");

    let mut shifted = spec(&f.ct, vec![segment(1, "A", a.clone())], None);
    shifted.geometry.origin_mm[0] += 2.5;
    write_seg(&path, &shifted).unwrap();
    let err = import_seg(&path, Some(&f.ct), &opts)
        .unwrap_err()
        .to_string();
    assert!(err.contains("lattice"), "{err}");

    let mut coarse = spec(&f.ct, vec![segment(1, "A", a.clone())], None);
    coarse.geometry.spacing_mm[0] = 10.0;
    write_seg(&path, &coarse).unwrap();
    assert!(import_seg(&path, Some(&f.ct), &opts).is_err());

    let mut other_series = spec(&f.ct, vec![segment(1, "A", a)], None);
    other_series.referenced_series_uid = Some("2.25.2".into());
    write_seg(&path, &other_series).unwrap();
    let err = import_seg(&path, Some(&f.ct), &opts)
        .unwrap_err()
        .to_string();
    assert!(err.contains("referenced series"), "{err}");

    // An RTSTRUCT is not a SEG.
    assert!(import_seg(&f.rtstruct, Some(&f.ct), &opts).is_err());
}

fn members(f: &Fixture, extra: &[&PathBuf]) -> Vec<(String, Vec<u8>)> {
    f.ct_files
        .iter()
        .chain(extra.iter().copied())
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                fs::read(p).unwrap(),
            )
        })
        .collect()
}

#[test]
fn ct_plus_seg_study_import_matches_rtstruct_outputs() {
    let f = fixture();
    let a = box_values(&f.ct, [10, 12, 5], [20, 22, 15], 1.0);
    let path = f.root.join("seg.dcm");
    write_seg(&path, &spec(&f.ct, vec![segment(1, "TUMOR", a)], None)).unwrap();

    let study = import_study_from_files(&members(&f, &[&path])).unwrap();
    assert_eq!(study.structure_source, StructureSource::Segmentation);
    assert_eq!(study.rois.len(), 1);
    assert_eq!(study.rois[0].name, "TUMOR");
    assert_eq!(study.rois[0].voxel_count, 1000);
    assert!((study.rois[0].volume_cm3 - 125.0).abs() < 1e-9);

    let mut paths = f.ct_files.clone();
    paths.push(path.clone());
    let import = import_ct_contours_from_paths(&paths).unwrap();
    assert_eq!(import.structure_source, Some(StructureSource::Segmentation));
    assert_eq!(import.structures.unwrap().rois[0].voxel_count(), 1000);
    assert!(import.members.iter().any(|(n, _)| n.ends_with("seg.dcm")));

    // The RTSTRUCT path is unchanged, and a SEG next to an RTSTRUCT is
    // refused as ambiguous.
    let rt = f.rtstruct.clone();
    let mut with_rt = f.ct_files.clone();
    with_rt.push(rt.clone());
    let import = import_ct_contours_from_paths(&with_rt).unwrap();
    assert_eq!(import.structure_source, Some(StructureSource::RtStruct));
    with_rt.push(path);
    let err = import_ct_contours_from_paths(&with_rt)
        .unwrap_err()
        .to_string();
    assert!(err.contains("multiple structure objects"), "{err}");
}

// SPDX-License-Identifier: MIT
//! NRRD / MetaImage readers against the same volume written as NIfTI.

use std::io::Write;
use std::path::Path;

use flate2::Compression;
use flate2::write::{GzEncoder, ZlibEncoder};
use openbnct_core::GridGeometry;
use openbnct_nifti::{
    NiftiImage, VolumeFormat, read_metaimage_file, read_nifti_file, read_nrrd_file, read_volume,
    sniff_volume_format, voxel_center, write_nifti,
};

fn truth() -> Vec<f64> {
    (0..12).map(|i| (i as f64) * 3.0 - 7.0).collect()
}

fn nifti_reference(dir: &Path, values: &[f64], spacing: [f64; 3], origin: [f64; 3]) -> NiftiImage {
    let image = NiftiImage {
        geometry: GridGeometry {
            shape: [3, 2, 2],
            spacing_mm: spacing,
            origin_mm: origin,
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        },
        values: values.to_vec(),
        datatype: 64,
        transform_source: "sform",
        description: String::new(),
        intent_name: String::new(),
        units_declared_mm: true,
    };
    let path = dir.join("ref.nii");
    write_nifti(&image, &path).unwrap();
    read_nifti_file(&path).unwrap()
}

fn assert_same(a: &NiftiImage, b: &NiftiImage) {
    assert_eq!(a.geometry.shape, b.geometry.shape);
    for i in 0..3 {
        assert!((a.geometry.spacing_mm[i] - b.geometry.spacing_mm[i]).abs() < 1e-9);
        assert!((a.geometry.origin_mm[i] - b.geometry.origin_mm[i]).abs() < 1e-9);
    }
    for (x, y) in a.geometry.direction.iter().zip(&b.geometry.direction) {
        assert!((x - y).abs() < 1e-9);
    }
    assert_eq!(a.values, b.values);
}

/// Encode `values` in the named signed/float type.
fn encode(ty: &str, values: &[f64], big: bool) -> Vec<u8> {
    let mut out = Vec::new();
    macro_rules! put {
        ($t:ty) => {
            for v in values {
                let x = *v as $t;
                out.extend_from_slice(&if big {
                    x.to_be_bytes()
                } else {
                    x.to_le_bytes()
                });
            }
        };
    }
    match ty {
        "int8" => put!(i8),
        "int16" => put!(i16),
        "int32" => put!(i32),
        "float" => put!(f32),
        "double" => put!(f64),
        _ => unreachable!(),
    }
    out
}

fn encode_unsigned(width: usize, values: &[f64], big: bool) -> Vec<u8> {
    values
        .iter()
        .flat_map(|v| {
            let x = *v as u32;
            if big {
                x.to_be_bytes()[4 - width..].to_vec()
            } else {
                x.to_le_bytes()[..width].to_vec()
            }
        })
        .collect()
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut e = GzEncoder::new(Vec::new(), Compression::fast());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

fn nrrd_header(ty: &str, enc: &str, endian: Option<&str>, extra: &str) -> String {
    let mut h = format!(
        "NRRD0004\ntype: {ty}\ndimension: 3\nspace: left-posterior-superior\nsizes: 3 2 2\n\
         space directions: (2,0,0) (0,1.5,0) (0,0,3)\nkinds: domain domain domain\nencoding: {enc}\n\
         space origin: (-10,20,30.5)\nspace units: \"mm\" \"mm\" \"mm\"\n"
    );
    if let Some(e) = endian {
        h.push_str(&format!("endian: {e}\n"));
    }
    h.push_str(extra);
    h
}

fn attach(header: String, payload: &[u8]) -> Vec<u8> {
    let mut bytes = header.into_bytes();
    bytes.push(b'\n');
    bytes.extend_from_slice(payload);
    bytes
}

#[test]
fn nrrd_types_endians_and_encodings_match_nifti() {
    let dir = tempfile::tempdir().unwrap();
    let want = nifti_reference(dir.path(), &truth(), [2.0, 1.5, 3.0], [-10.0, 20.0, 30.5]);
    let path = dir.path().join("a.nrrd");
    for ty in ["int8", "int16", "int32", "float", "double"] {
        for big in [false, true] {
            let endian = if big { "big" } else { "little" };
            let raw = encode(ty, &truth(), big);
            std::fs::write(
                &path,
                attach(nrrd_header(ty, "raw", Some(endian), ""), &raw),
            )
            .unwrap();
            assert_same(&read_nrrd_file(&path).unwrap(), &want);
            assert_same(&read_volume(&path).unwrap(), &want);
            let zipped = gzip(&raw);
            std::fs::write(
                &path,
                attach(nrrd_header(ty, "gzip", Some(endian), ""), &zipped),
            )
            .unwrap();
            assert_same(&read_nrrd_file(&path).unwrap(), &want);
        }
    }
    let pos: Vec<f64> = (0..12).map(|i| i as f64 * 5.0).collect();
    let want_pos = nifti_reference(dir.path(), &pos, [2.0, 1.5, 3.0], [-10.0, 20.0, 30.5]);
    for (ty, width) in [("uint8", 1), ("uint16", 2), ("uint32", 4)] {
        for big in [false, true] {
            let endian = if big { "big" } else { "little" };
            let raw = encode_unsigned(width, &pos, big);
            std::fs::write(
                &path,
                attach(nrrd_header(ty, "raw", Some(endian), ""), &raw),
            )
            .unwrap();
            assert_same(&read_nrrd_file(&path).unwrap(), &want_pos);
        }
    }
}

#[test]
fn nrrd_ascii_and_detached() {
    let dir = tempfile::tempdir().unwrap();
    let want = nifti_reference(dir.path(), &truth(), [2.0, 1.5, 3.0], [-10.0, 20.0, 30.5]);
    let text: Vec<String> = truth().iter().map(|v| format!("{v}")).collect();
    let path = dir.path().join("t.nrrd");
    std::fs::write(
        &path,
        attach(
            nrrd_header("short", "ascii", None, ""),
            text.join(" ").as_bytes(),
        ),
    )
    .unwrap();
    assert_same(&read_nrrd_file(&path).unwrap(), &want);

    // Detached header + raw data file with a byte skip.
    let mut data = vec![0xEE_u8; 5];
    data.extend(encode("int16", &truth(), false));
    std::fs::write(dir.path().join("vol.raw"), &data).unwrap();
    let header = nrrd_header(
        "int16",
        "raw",
        Some("little"),
        "data file: vol.raw\nbyte skip: 5\n",
    );
    let hdr = dir.path().join("vol.nhdr");
    std::fs::write(&hdr, header).unwrap();
    assert_same(&read_nrrd_file(&hdr).unwrap(), &want);
    assert_eq!(sniff_volume_format(&hdr).unwrap(), VolumeFormat::Nrrd);
    assert_same(&read_volume(&hdr).unwrap(), &want);

    // Detached gzip data.
    std::fs::write(
        dir.path().join("vol.raw.gz"),
        gzip(&encode("int16", &truth(), false)),
    )
    .unwrap();
    std::fs::write(
        &hdr,
        nrrd_header("int16", "gzip", Some("little"), "data file: vol.raw.gz\n"),
    )
    .unwrap();
    assert_same(&read_nrrd_file(&hdr).unwrap(), &want);

    // Detached files may not escape the header directory.
    let evil = nrrd_header("int16", "raw", Some("little"), "data file: ../vol.raw\n");
    std::fs::write(&hdr, evil).unwrap();
    assert!(read_nrrd_file(&hdr).is_err());
}

#[test]
fn nrrd_ras_flip_permutation_and_oblique() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.nrrd");
    let raw = encode("float", &truth(), false);
    let want = nifti_reference(dir.path(), &truth(), [2.0, 1.5, 3.0], [-10.0, 20.0, 30.5]);

    // RAS space with x/y negated is the same physical volume as LPS identity.
    let header = "NRRD0004\ntype: float\ndimension: 3\nspace: right-anterior-superior\n\
        sizes: 3 2 2\nspace directions: (-2,0,0) (0,-1.5,0) (0,0,3)\nencoding: raw\n\
        endian: little\nspace origin: (10,-20,30.5)\n";
    std::fs::write(&path, attach(header.into(), &raw)).unwrap();
    assert_same(&read_nrrd_file(&path).unwrap(), &want);

    // Flipped x in LPS: axis 0 runs toward -x starting at x = -6.
    let header = "NRRD0004\ntype: float\ndimension: 3\nspace: LPS\nsizes: 3 2 2\n\
        space directions: (-2,0,0) (0,1.5,0) (0,0,3)\nencoding: raw\nendian: little\n\
        space origin: (-6,20,30.5)\n";
    std::fs::write(&path, attach(header.into(), &raw)).unwrap();
    let got = read_nrrd_file(&path).unwrap();
    assert_eq!(got.geometry.origin_mm, [-10.0, 20.0, 30.5]);
    for k in 0..2 {
        for j in 0..2 {
            for i in 0..3 {
                let src = i + 3 * j + 6 * k;
                let mirrored = (2 - i) + 3 * j + 6 * k;
                assert_eq!(got.values[mirrored], truth()[src]);
            }
        }
    }

    // Permutation: axis 0 along y, axis 1 along x. Shape becomes [2,3,2].
    let header = "NRRD0004\ntype: float\ndimension: 3\nspace: LPS\nsizes: 3 2 2\n\
        space directions: (0,1,0) (2,0,0) (0,0,3)\nencoding: raw\nendian: little\n\
        space origin: (0,0,0)\n";
    std::fs::write(&path, attach(header.into(), &raw)).unwrap();
    let got = read_nrrd_file(&path).unwrap();
    assert_eq!(got.geometry.shape, [2, 3, 2]);
    assert_eq!(got.geometry.spacing_mm, [2.0, 1.0, 3.0]);
    assert_eq!(voxel_center(&got.geometry, 1, 2, 1), [2.0, 2.0, 3.0]);
    // New (i=1, j=2, k=1) is old (i=2 along y, j=1 along x, k=1).
    assert_eq!(got.values[1 + 2 * 2 + 6], truth()[2 + 3 + 6]);

    // Oblique is refused.
    let header = "NRRD0004\ntype: float\ndimension: 3\nspace: LPS\nsizes: 3 2 2\n\
        space directions: (1,0.3,0) (0,1,0) (0,0,1)\nencoding: raw\nendian: little\n";
    std::fs::write(&path, attach(header.into(), &raw)).unwrap();
    let err = read_nrrd_file(&path).unwrap_err().to_string();
    assert!(err.contains("oblique"), "{err}");
}

fn mha_header(ty: &str, extra: &str, data: &str) -> String {
    format!(
        "ObjectType = Image\nNDims = 3\nBinaryData = True\n{extra}\
         TransformMatrix = 1 0 0 0 1 0 0 0 1\nOffset = -10 20 30.5\n\
         ElementSpacing = 2 1.5 3\nDimSize = 3 2 2\nElementType = {ty}\nElementDataFile = {data}\n"
    )
}

#[test]
fn metaimage_types_endians_compression_and_detached() {
    let dir = tempfile::tempdir().unwrap();
    let want = nifti_reference(dir.path(), &truth(), [2.0, 1.5, 3.0], [-10.0, 20.0, 30.5]);
    let path = dir.path().join("a.mha");
    for (ty, nty) in [
        ("MET_CHAR", "int8"),
        ("MET_SHORT", "int16"),
        ("MET_INT", "int32"),
        ("MET_FLOAT", "float"),
        ("MET_DOUBLE", "double"),
    ] {
        for big in [false, true] {
            let raw = encode(nty, &truth(), big);
            let msb = format!(
                "ElementByteOrderMSB = {}\n",
                if big { "True" } else { "False" }
            );
            let mut bytes = mha_header(ty, &msb, "LOCAL").into_bytes();
            bytes.extend_from_slice(&raw);
            std::fs::write(&path, &bytes).unwrap();
            assert_same(&read_metaimage_file(&path).unwrap(), &want);
            assert_same(&read_volume(&path).unwrap(), &want);

            let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
            z.write_all(&raw).unwrap();
            let z = z.finish().unwrap();
            let extra = format!(
                "{msb}CompressedData = True\nCompressedDataSize = {}\n",
                z.len()
            );
            let mut bytes = mha_header(ty, &extra, "LOCAL").into_bytes();
            bytes.extend_from_slice(&z);
            std::fs::write(&path, &bytes).unwrap();
            assert_same(&read_metaimage_file(&path).unwrap(), &want);

            std::fs::write(dir.path().join("v.raw"), &raw).unwrap();
            let mhd = dir.path().join("v.mhd");
            std::fs::write(&mhd, mha_header(ty, &msb, "v.raw")).unwrap();
            assert_same(&read_metaimage_file(&mhd).unwrap(), &want);
            assert_eq!(sniff_volume_format(&mhd).unwrap(), VolumeFormat::MetaImage);
        }
    }
    let pos: Vec<f64> = (0..12).map(|i| i as f64 * 5.0).collect();
    let want_pos = nifti_reference(dir.path(), &pos, [2.0, 1.5, 3.0], [-10.0, 20.0, 30.5]);
    for (ty, width) in [("MET_UCHAR", 1), ("MET_USHORT", 2), ("MET_UINT", 4)] {
        let mut bytes = mha_header(ty, "", "LOCAL").into_bytes();
        bytes.extend_from_slice(&encode_unsigned(width, &pos, false));
        std::fs::write(&path, &bytes).unwrap();
        assert_same(&read_metaimage_file(&path).unwrap(), &want_pos);
    }
}

#[test]
fn metaimage_flip_and_oblique() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.mha");
    let raw = encode("float", &truth(), false);
    let header = "ObjectType = Image\nNDims = 3\nBinaryData = True\nElementByteOrderMSB = False\n\
        TransformMatrix = 1 0 0 0 1 0 0 0 -1\nOffset = -10 20 33.5\nElementSpacing = 2 1.5 3\n\
        DimSize = 3 2 2\nElementType = MET_FLOAT\nElementDataFile = LOCAL\n";
    let mut bytes = header.as_bytes().to_vec();
    bytes.extend_from_slice(&raw);
    std::fs::write(&path, &bytes).unwrap();
    let got = read_metaimage_file(&path).unwrap();
    assert_eq!(got.geometry.origin_mm, [-10.0, 20.0, 30.5]);
    assert_eq!(got.values[0], truth()[6]);
    assert_eq!(got.values[6], truth()[0]);

    let oblique = header.replace("1 0 0 0 1 0 0 0 -1", "0.8 0.6 0 -0.6 0.8 0 0 0 1");
    let mut bytes = oblique.into_bytes();
    bytes.extend_from_slice(&raw);
    std::fs::write(&path, &bytes).unwrap();
    assert!(
        read_metaimage_file(&path)
            .unwrap_err()
            .to_string()
            .contains("oblique")
    );
}

#[test]
fn read_volume_sniffs_nifti_and_rejects_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let want = nifti_reference(dir.path(), &truth(), [2.0, 1.5, 3.0], [-10.0, 20.0, 30.5]);
    let odd = dir.path().join("scan.dat");
    std::fs::copy(dir.path().join("ref.nii"), &odd).unwrap();
    assert_same(&read_volume(&odd).unwrap(), &want);
    let junk = dir.path().join("junk.bin");
    std::fs::write(&junk, b"hello").unwrap();
    assert!(read_volume(&junk).is_err());
    // Truncated NRRD data is an error, not a short volume.
    let t = dir.path().join("t.nrrd");
    std::fs::write(
        &t,
        attach(nrrd_header("float", "raw", Some("little"), ""), &[0_u8; 10]),
    )
    .unwrap();
    assert!(read_volume(&t).is_err());
}

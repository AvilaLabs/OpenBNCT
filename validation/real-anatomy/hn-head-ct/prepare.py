#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Fetch one public head-and-neck planning CT and convert it for OpenBNCT.

Source data: google-deepmind/tcia-ct-scan-dataset (CC BY 4.0), test set,
oncologist-arbitrated segmentations, patient folder 0522c0226. Every file is
fetched from a commit-pinned raw URL and checked against the sha256 in
`fetch-pins.json`; nothing is fetched or trusted otherwise.

Output (in --out, default ./data; regenerated, not committed):
  hu.nii.gz        CT in HU, float32, axis-aligned LPS grid, 0.98 x 0.98 x 2.5 mm,
                   cropped to head and neck (see "Crop" below)
  labels.nii.gz    int32 bitmask labelmap (one bit per ROI; ROIs may overlap)
  label-names.json {"encoding": "bitmask", "labels": {"1": "Brain", ...}}
  derived.json     the derivation record (target centre, volumes, hashes)

Derived structures (all computed here from the CT and the radiographer labels):
  BODY             HU > -400, largest 6-connected component, holes filled
                   (per axial slice, then in 3-D)
  SKIN             the outer 5 mm shell of BODY (Euclidean distance to the
                   outside of BODY <= 5 mm)
  RESEARCH_TARGET  SYNTHETIC. A 25 mm radius sphere, intersected with Brain,
                   centred 35 mm inside the scalp surface along the beam axis.
                   It is a geometric stand-in for a lesion, NOT a tumor.

Crop. The full 512 x 512 x 162 field of view (torso included) is 1.6 M voxels at
4 mm and its transport step exceeds the 7 GiB memory cap the benchmark runs
under. The CT is cropped (voxel-exact, no resampling) to 20 mm below the mandible
upward and to the BODY bounding box + 15 mm of air in x and y. Structures are
derived on the full CT first; those left empty by the crop (the lungs) are dropped.

Orientation. NRRD stores `space directions` (one world vector per array axis)
and `space origin` in the named `space`. Here the space is
left-posterior-superior (LPS) with diagonal directions, so no resampling is
needed. The code is general: RAS spaces are converted to LPS; each array axis
must lie along exactly one world axis (else it stops); axes are permuted to
(x, y, z) and flipped to increasing coordinate. The NIfTI affine (RAS+) is then
diag(-sx, -sy, sz) with translation (-ox, -oy, oz), which OpenBNCT reads back as
an identity-direction LPS grid with the same origin. Nothing is interpolated.
"""

import argparse
import hashlib
import json
import sys
import time
import urllib.request
from pathlib import Path

import nibabel as nib
import nrrd
import numpy as np
from scipy import ndimage

HERE = Path(__file__).resolve().parent
PATIENT = "0522c0226"
SEGMENT_ROOT = f"nrrds/test/oncologist/{PATIENT}/segmentations/"
CT_PATH = f"nrrds/test/oncologist/{PATIENT}/CT_IMAGE.nrrd"

TARGET_RADIUS_MM = 25.0
TARGET_DEPTH_MM = 35.0
SKIN_MM = 5.0
CROP_BELOW_MANDIBLE_MM = 20.0
CROP_MARGIN_MM = 15.0
BODY_HU = -400.0
# Beam travel direction in LPS: +x, i.e. entering through the patient's right
# scalp (low x) and travelling toward the left.
APPROACH = "+x"


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def fetch(pins, cache):
    """Download every pinned file into `cache`, verifying size and sha256."""
    for entry in pins["files"]:
        dest = cache / entry["path"]
        if dest.exists() and sha256(dest) == entry["sha256"]:
            continue
        dest.parent.mkdir(parents=True, exist_ok=True)
        url = (
            f"https://raw.githubusercontent.com/{pins['repo']}/"
            f"{pins['commit']}/{entry['path']}"
        )
        for attempt in range(8):
            try:
                data = urllib.request.urlopen(url, timeout=120).read()
                break
            except Exception as error:  # transient network failure
                print(f"retry {attempt}: {url}: {error}", file=sys.stderr)
                time.sleep(2 + 3 * attempt)
        else:
            sys.exit(f"could not fetch {url}")
        if hashlib.sha256(data).hexdigest() != entry["sha256"] or len(data) != entry["bytes"]:
            sys.exit(f"{entry['path']}: sha256/size differs from fetch-pins.json")
        dest.write_bytes(data)
        print(f"fetched {entry['path']} ({len(data)} bytes)")


def read_canonical(path):
    """Read an NRRD onto a canonical LPS grid.

    Returns (array in (x, y, z) order, spacing[3], origin[3] = centre of voxel
    (0,0,0) in LPS mm). Raises if the grid is not axis-aligned.
    """
    array, header = nrrd.read(str(path))
    if array.ndim != 3:
        sys.exit(f"{path}: expected 3-D data")
    directions = np.array(header["space directions"], dtype=float)  # row i = array axis i
    origin = np.array(header["space origin"], dtype=float)
    space = header.get("space", "left-posterior-superior")
    if space in ("right-anterior-superior", "RAS"):
        flip = np.array([-1.0, -1.0, 1.0])
        directions = directions * flip
        origin = origin * flip
    elif space not in ("left-posterior-superior", "LPS"):
        sys.exit(f"{path}: unsupported NRRD space {space!r}")
    axis_of = []
    signs = []
    for i in range(3):
        v = directions[i]
        a = int(np.argmax(np.abs(v)))
        if np.abs(np.delete(v, a)).max() > 1e-3 * np.abs(v[a]):
            sys.exit(f"{path}: array axis {i} is oblique; resample to axis-aligned first")
        axis_of.append(a)
        signs.append(np.sign(v[a]))
    if sorted(axis_of) != [0, 1, 2]:
        sys.exit(f"{path}: two array axes map to the same world axis")
    # Permute so output axis a is the array axis that lies along world axis a.
    perm = [axis_of.index(a) for a in range(3)]
    array = np.transpose(array, perm)
    spacing = np.array([abs(directions[perm[a]][a]) for a in range(3)])
    step_sign = np.array([signs[perm[a]] for a in range(3)])
    for a in range(3):
        if step_sign[a] < 0:
            array = np.flip(array, axis=a)
            origin[a] = origin[a] + (array.shape[a] - 1) * directions[perm[a]][a]
    return np.ascontiguousarray(array), spacing, origin


def write_nifti(path, array, spacing, origin, dtype):
    """Write an axis-aligned LPS grid as NIfTI-1 (RAS+ sform)."""
    affine = np.array(
        [
            [-spacing[0], 0, 0, -origin[0]],
            [0, -spacing[1], 0, -origin[1]],
            [0, 0, spacing[2], origin[2]],
            [0, 0, 0, 1.0],
        ]
    )
    image = nib.Nifti1Image(array.astype(dtype), affine)
    image.set_sform(affine, code=2)
    image.set_qform(affine, code=2)
    image.header.set_xyzt_units("mm")
    nib.save(image, str(path))


def largest_component(mask):
    labels, count = ndimage.label(mask)  # 6-connectivity
    if count == 0:
        sys.exit("BODY threshold selected no voxels")
    sizes = ndimage.sum(mask, labels, index=np.arange(1, count + 1))
    return labels == (1 + int(np.argmax(sizes)))


def make_body(hu):
    body = largest_component(hu > BODY_HU)
    for k in range(body.shape[2]):
        body[:, :, k] = ndimage.binary_fill_holes(body[:, :, k])
    return ndimage.binary_fill_holes(body)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--cache", type=Path, default=HERE / "cache", help="download cache")
    parser.add_argument("--out", type=Path, default=HERE / "data", help="output directory")
    args = parser.parse_args()

    pins = json.loads((HERE / "fetch-pins.json").read_text())
    fetch(pins, args.cache)
    args.out.mkdir(parents=True, exist_ok=True)

    hu, spacing, origin = read_canonical(args.cache / CT_PATH)
    print(f"CT {hu.shape} spacing {spacing} origin {origin} HU [{hu.min():.0f}, {hu.max():.0f}]")

    structures = {}
    for entry in sorted(pins["files"], key=lambda e: e["path"]):
        if not entry["path"].startswith(SEGMENT_ROOT):
            continue
        name = Path(entry["path"]).stem
        seg, s, o = read_canonical(args.cache / entry["path"])
        if seg.shape != hu.shape or not np.allclose(s, spacing) or not np.allclose(o, origin, atol=1e-3):
            sys.exit(f"{name}: segmentation grid differs from the CT grid")
        structures[name] = seg > 0

    body = make_body(hu)
    dist = ndimage.distance_transform_edt(body, sampling=spacing)
    skin = body & (dist <= SKIN_MM)

    # Synthetic target: beam travels +x, so it enters through the low-x
    # (patient right) scalp. Aim along the line through the brain centroid's
    # (y, z); scalp surface = first BODY voxel face along +x on that line.
    brain = structures["Brain"]
    centroid = np.array(np.nonzero(brain)).mean(axis=1)
    jy, kz = int(round(centroid[1])), int(round(centroid[2]))
    line = np.nonzero(body[:, jy, kz])[0]
    if line.size == 0:
        sys.exit("beam line misses BODY")
    surface_x = origin[0] + (line[0] - 0.5) * spacing[0]
    center = np.array(
        [surface_x + TARGET_DEPTH_MM, origin[1] + jy * spacing[1], origin[2] + kz * spacing[2]]
    )
    axes = [origin[a] + np.arange(hu.shape[a]) * spacing[a] for a in range(3)]
    gx, gy, gz = np.meshgrid(*axes, indexing="ij")
    ball = (gx - center[0]) ** 2 + (gy - center[1]) ** 2 + (gz - center[2]) ** 2 <= TARGET_RADIUS_MM**2
    target = ball & brain
    if not target[tuple(np.round((center - origin) / spacing).astype(int))]:
        sys.exit("target centre is not inside Brain")

    structures["BODY"] = body
    structures["SKIN"] = skin
    structures["RESEARCH_TARGET"] = target

    # Crop to head and neck. The transport step at the full 512 x 512 x 162 field of
    # view (1.6 M voxels at 4 mm) exceeds the 7 GiB memory cap this benchmark is run
    # under, and the torso is irrelevant to a head irradiation. z: 20 mm below the
    # mandible up; x, y: BODY bounding box in that z range plus a margin of air. All
    # structures and BODY/SKIN were derived on the full CT first (so the cut plane is
    # not SKIN). Structures with no voxel left (the lungs) are dropped.
    z_lo = max(0, int(np.nonzero(structures["Mandible"].any(axis=(0, 1)))[0][0]) - int(round(CROP_BELOW_MANDIBLE_MM / spacing[2])))
    z_sel = np.zeros(hu.shape, dtype=bool)
    z_sel[:, :, z_lo:] = True
    box = np.nonzero(body & z_sel)
    margin = [int(round(CROP_MARGIN_MM / spacing[a])) for a in range(3)]
    lo = [max(0, int(box[a].min()) - margin[a]) for a in range(2)] + [z_lo]
    hi = [min(hu.shape[a], int(box[a].max()) + 1 + margin[a]) for a in range(2)] + [hu.shape[2]]
    crop = tuple(slice(lo[a], hi[a]) for a in range(3))
    print(f"crop index box {lo} .. {hi} of {hu.shape}")
    hu = hu[crop]
    origin = origin + np.array(lo) * spacing
    structures = {n: m[crop] for n, m in structures.items()}
    ball = ball[crop]
    dropped = [n for n, m in structures.items() if not m.any()]
    structures = {n: m for n, m in structures.items() if m.any()}
    print("dropped (empty after crop):", dropped)

    names = sorted(n for n in structures if n not in ("BODY", "SKIN", "RESEARCH_TARGET"))
    names += ["BODY", "SKIN", "RESEARCH_TARGET"]
    labelmap = np.zeros(hu.shape, dtype=np.int32)
    labels = {}
    for bit, name in enumerate(names):
        labelmap |= structures[name].astype(np.int32) << bit
        labels[str(1 << bit)] = name
    label_doc = {
        "encoding": "bitmask",
        "description": (
            "Organ-at-risk labels are the radiographer/oncologist segmentations of the "
            "google-deepmind/tcia-ct-scan-dataset (CC BY 4.0). BODY, SKIN and RESEARCH_TARGET "
            "are derived by prepare.py. RESEARCH_TARGET is a SYNTHETIC sphere for exercising "
            "the workflow; it is not a tumor and represents no patient finding."
        ),
        "labels": labels,
    }
    write_nifti(args.out / "hu.nii.gz", hu, spacing, origin, np.float32)
    write_nifti(args.out / "labels.nii.gz", labelmap, spacing, origin, np.int32)
    (args.out / "label-names.json").write_text(json.dumps(label_doc, indent=1) + "\n")

    voxel_cm3 = float(np.prod(spacing)) / 1000.0
    derived = {
        "scan": f"tcia-ct-scan-dataset {PATIENT} (test, oncologist segmentations)",
        "grid": {"shape": list(hu.shape), "spacing_mm": spacing.tolist(), "origin_lps_mm": origin.tolist()},
        "beam_approach": APPROACH,
        "scalp_entry_x_mm": float(surface_x),
        "target_center_lps_mm": center.tolist(),
        "target_radius_mm": TARGET_RADIUS_MM,
        "target_depth_mm": TARGET_DEPTH_MM,
        "target_volume_cm3": float(target.sum()) * voxel_cm3,
        "target_ball_volume_cm3": float(ball.sum()) * voxel_cm3,
        "cropped_index_box": {"lo": lo, "hi": hi},
        "dropped_empty_after_crop": dropped,
        "structure_volumes_cm3": {n: float(structures[n].sum()) * voxel_cm3 for n in names},
        "outputs_sha256": {
            f: sha256(args.out / f) for f in ("hu.nii.gz", "labels.nii.gz", "label-names.json")
        },
    }
    (args.out / "derived.json").write_text(json.dumps(derived, indent=1) + "\n")
    print(json.dumps({k: derived[k] for k in ("scalp_entry_x_mm", "target_center_lps_mm", "target_volume_cm3", "target_ball_volume_cm3")}, indent=1))
    for n in names:
        print(f"  {n:18s} {derived['structure_volumes_cm3'][n]:9.2f} cm3")


if __name__ == "__main__":
    main()

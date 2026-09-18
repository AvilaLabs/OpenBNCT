#!/usr/bin/env python3
"""Generate the layered spherical head phantom benchmark artifacts.

Concentric-sphere "Snyder-style" head: skin shell, cortical-skull shell,
brain core — ICRU-44 elemental compositions, natural isotopic splits —
voxelized on an 8 mm Cartesian grid inside a 20 cm cube. Beam model is
the FiR 1 K63 disk + 8.5 deg isotropic cone (same fixed-source artifact
as the cylindrical-phantom validation).

Shell thicknesses are sized to the mesh (8 mm each) so every tissue
layer is voxel-resolved; this is a declared research benchmark, not an
anatomical replica.

Outputs (run from the repository root):
    benchmarks/synthetic/layered-head-phantom/{case,assignment}.json
    benchmarks/synthetic/layered-head-phantom/materials/{skin,skull,brain,void}.json
"""

import json
import math
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
SPACING_MM = 8.0
GRID = 25                       # cells per axis -> 20 cm cube
EXTENT_MM = GRID * SPACING_MM   # 200 mm
# Beam axis through the center of voxel column (12,12) in x,y.
ORIGIN_XY_MM = -100.0           # voxel 12 spans [-4, 4] mm -> centered on 0
CENTER = (0.0, 0.0, 100.0)      # sphere center, mm (z axis: beam direction)
R_OUT = 100.0                   # skin outer radius
R_SKULL = 84.0                  # skull outer radius (8 mm skin shell)
R_BRAIN = 76.0                  # brain radius (8 mm skull shell)

# ICRU-44 elemental mass fractions.
SKIN = {"H": 0.100, "C": 0.204, "N": 0.042, "O": 0.645,
        "Na": 0.002, "P": 0.001, "S": 0.002, "Cl": 0.003, "K": 0.001}
SKULL = {"H": 0.034, "C": 0.155, "N": 0.042, "O": 0.435, "Na": 0.001,
         "Mg": 0.002, "P": 0.103, "S": 0.003, "Ca": 0.225}
BRAIN = {"H": 0.107, "C": 0.145, "N": 0.022, "O": 0.712, "Na": 0.002,
         "P": 0.004, "S": 0.002, "Cl": 0.003, "K": 0.003}
DENSITIES = {"skin": 1.09, "skull": 1.92, "brain": 1.04}

# Natural abundances; minor isotopes below 1e-4 abundance are dropped
# and the remainder renormalized (S36, Ca46 carried anyway since tapes
# are extracted).
ISOTOPES = {
    "H":  {"H1": 0.999885, "H2": 0.000115},
    "C":  {"C12": 0.9893, "C13": 0.0107},
    "N":  {"N14": 0.99636, "N15": 0.00364},
    "O":  {"O16": 0.99757, "O17": 0.00038, "O18": 0.00205},
    "Na": {"Na23": 1.0},
    "Mg": {"Mg24": 0.7899, "Mg25": 0.10, "Mg26": 0.1101},
    "P":  {"P31": 1.0},
    "S":  {"S32": 0.9499, "S33": 0.0075, "S34": 0.0425},
    "Cl": {"Cl35": 0.7578, "Cl37": 0.2422},
    "K":  {"K39": 0.932581, "K40": 0.000117, "K41": 0.067302},
    "Ca": {"Ca40": 0.96941, "Ca42": 0.00647, "Ca43": 0.00135,
           "Ca44": 0.02086, "Ca46": 0.00004, "Ca48": 0.00187},
}

# Trace 10B in brain — dose-response activation without perturbing the
# neutron field (same convention as the water-phantom material).
B10_TRACE = 1.0e-7


def material(mat_id, density, elements, boron=0.0):
    nuclides = []
    total_boron = boron
    scale = 1.0 - total_boron
    for elem, frac in elements.items():
        for iso, abund in ISOTOPES[elem].items():
            nuclides.append({"name": iso, "mass_fraction": frac * scale * abund})
    if boron > 0.0:
        nuclides.append({"name": "B10", "mass_fraction": boron})
    # Isotopic abundances carry evaluated rounding (sum ≈ 1 ± 1e-7); fold
    # the residual into the largest fraction so the total is exactly one.
    residual = 1.0 - sum(n["mass_fraction"] for n in nuclides)
    largest = max(nuclides, key=lambda n: n["mass_fraction"])
    largest["mass_fraction"] += residual
    return {
        "schema_version": "openbnct.material-definition/0.1.0",
        "id": mat_id,
        "density_g_cm3": density,
        "temperature_k": 293.6,
        "nuclides": nuclides,
        "neutron_thermal_treatment": "free_gas",
    }


def voxel_indices(lo_r, hi_r):
    """Voxel-set indices whose centers sit in the shell lo_r < r <= hi_r."""
    out = []
    cx, cy, cz = CENTER
    for k in range(GRID):
        z = ORIGIN_Z + (k + 0.5) * SPACING_MM - cz
        for j in range(GRID):
            y = ORIGIN_XY_MM + (j + 0.5) * SPACING_MM - cy
            for i in range(GRID):
                x = ORIGIN_XY_MM + (i + 0.5) * SPACING_MM - cx
                r = math.sqrt(x * x + y * y + z * z)
                if lo_r < r <= hi_r:
                    out.append([i, j, k])
    return out


ORIGIN_Z = 0.0  # sphere spans z in [0, 200] mm

skin_vox = voxel_indices(R_SKULL, R_OUT)
skull_vox = voxel_indices(R_BRAIN, R_SKULL)
brain_vox = voxel_indices(-1.0, R_BRAIN)

mat_dir = ROOT / "materials"
mat_dir.mkdir(parents=True, exist_ok=True)
mats = {
    "skin": material("openbnct.layered-head.skin.v1", DENSITIES["skin"], SKIN),
    "skull": material("openbnct.layered-head.skull.v1", DENSITIES["skull"], SKULL),
    "brain": material("openbnct.layered-head.brain.v1", DENSITIES["brain"], BRAIN,
                      boron=B10_TRACE),
    "void": material("openbnct.layered-head.void.v1", 1.0e-9, {"H": 1.0}),
}
for name, m in mats.items():
    with open(mat_dir / f"{name}.json", "w") as fh:
        json.dump(m, fh, indent=1)

# The transport-case convention takes origin_mm as the first voxel
# CENTER (grid faces sit at origin − spacing/2), while ORIGIN_* here
# name the low faces — shift by half a cell.
geometry = {
    "shape": [GRID, GRID, GRID],
    "spacing_mm": [SPACING_MM] * 3,
    "origin_mm": [ORIGIN_XY_MM + 0.5 * SPACING_MM,
                  ORIGIN_XY_MM + 0.5 * SPACING_MM,
                  ORIGIN_Z + 0.5 * SPACING_MM],
    "direction": [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
}

source = {
    "schema_version": "openbnct.fixed-source-definition/0.1.0",
    "id": "nctforge.beam.fir1-k63.v1",
    "particle": "neutron",
    "source_sites_per_history": 1,
    "statistical_weight_per_site": 1.0,
    "space": {"kind": "uniform_disk", "axis": "z", "offset_cm": 0.0,
              "center_uv_cm": [0.0, 0.0], "radius_cm": 7.0},
    "angle": {"kind": "isotropic_cone",
              "axis_unit_vector": [0.0, 0.0, 1.0],
              "half_angle_rad": 0.1491},
    "energy": {"kind": "tabulated_histogram",
               "energy_boundaries_ev": [1e-5, 0.5, 1e4, 1.69e7],
               "bin_weights": [0.0611, 0.9093, 0.0296]},
}

case = {
    "schema_version": "openbnct.transport-case/0.1.0",
    "case_id": "openbnct.layered-head-phantom.v1",
    "geometry": geometry,
    "material": mats["void"],
    "source": source,
    "requested_histories": 1,
}
with open(ROOT / "case.json", "w") as fh:
    json.dump(case, fh, indent=1)

assignment = {
    "schema_version": "openbnct.material-assignment/0.2.0",
    "case_id": "openbnct.layered-head-phantom.v1",
    "base_material": mats["void"],
    "regions": [
        {"name": "skin-shell", "material": mats["skin"],
         "shape": {"kind": "voxel_set", "indices": skin_vox}},
        {"name": "skull-shell", "material": mats["skull"],
         "shape": {"kind": "voxel_set", "indices": skull_vox}},
        {"name": "brain-core", "material": mats["brain"],
         "shape": {"kind": "voxel_set", "indices": brain_vox}},
    ],
    "provenance_id": "openbnct.layered-head-phantom.generator.v1",
}
with open(ROOT / "assignment.json", "w") as fh:
    json.dump(assignment, fh, indent=1)

print(f"skin {len(skin_vox)}  skull {len(skull_vox)}  brain {len(brain_vox)}")
print(f"total phantom voxels: {len(skin_vox) + len(skull_vox) + len(brain_vox)}")

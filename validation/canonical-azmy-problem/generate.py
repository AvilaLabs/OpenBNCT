#!/usr/bin/env python3
"""Generate the canonical Azmy (1988) weighted-DD case.

Full-domain realization of Azmy's quadrant problem (Azmy 1988, "The
Weighted Diamond-Difference Form of Nodal Transport Methods," NSE
98:29-40 — the formulation used by FeenoX's neutron_sn examples). The
quarter-domain has mirror BCs on x=0,y=0 —
realized here by solving the full square x,y in [-10,10] cm with
vacuum on all four faces; reflection equivariance makes each family
of four mirrored quadrants identical, so the published quadrant means
apply directly:

    |x|<5, |y|<5   source     sigma_t=1, sigma_s=0.5, q=1
    elsewhere       absorber   sigma_t=2, sigma_s=0.1, q=0

Published quadrant means (Azmy 1988):
    source quadrant       1.676
    edge-adjacent         4.159e-2
    corner (diagonal)     1.992e-3

The transport is planar, so z is a thin periodic slab (infinite
extrusion of the 2-D problem).

Emit:
  multigroup-data.json - one group, two materials (src, abs)
  assignment.json      - voxel_box region for the source square
  case.json            - UniformBox source covering |x|<5, |y|<5
"""

import json
import os

OUT = os.environ.get("AZMY_OUT", os.path.dirname(os.path.abspath(__file__)))

# In-plane cells of 200/NXY mm covering [-10,10] cm per axis; AZMY_NXY
# refines the in-plane mesh (material boundary at |x|=|y|=5 must stay
# on a cell edge, so NXY must be a multiple of 4).
NXY = int(os.environ.get("AZMY_NXY", "80"))
NZ = 2
SPACING_MM = [200.0 / NXY, 200.0 / NXY, 2.5]
ORIGIN_MM = [
    -100.0 + SPACING_MM[0] / 2.0,
    -100.0 + SPACING_MM[1] / 2.0,
    -SPACING_MM[2] / 2.0,
]
Z_CM = NZ * SPACING_MM[2] / 10.0  # transverse thickness


def material(mid):
    return {
        "schema_version": "openbnct.material-definition/0.1.0",
        "id": mid,
        "density_g_cm3": 1.0,
        "temperature_k": 294.0,
        "nuclides": [{"name": "X1", "mass_fraction": 1.0}],
        "neutron_thermal_treatment": "free_gas",
    }


def multigroup_data():
    def mat(mid, sigma_t, sigma_s):
        return {
            "material_id": mid,
            "sigma_total_per_cm": [sigma_t],
            "scatter_matrix_per_cm": [sigma_s],
            "dose_response_gy_cm2": {},
            "transport_mu_bar": [0.0],
        }

    return {
        "schema_version": "openbnct.multigroup-data/0.1.0",
        "id": "azmy-problem-1g",
        "energy_boundaries_ev": [1.0, 1.0e-3],
        "collapse_declaration": "Canonical one-group benchmark data "
        "(Azmy 1988 NSE 98:29-40 weighted-DD problem): declared "
        "macroscopic sigma_t / sigma_s0, isotropic scatter.",
        "component_profile": None,
        "materials": [
            mat("azmy.src", 1.0, 0.5),
            mat("azmy.abs", 2.0, 0.1),
        ],
    }


def axis_cells(lo_cm, hi_cm, axis):
    """Cell indices along axis whose centers lie in [lo, hi) cm."""
    out = []
    for i in range(NXY):
        c_cm = (ORIGIN_MM[axis] + SPACING_MM[axis] * i) / 10.0
        if lo_cm - 1e-12 <= c_cm < hi_cm - 1e-12:
            out.append(i)
    return out


def assignment():
    src_x = axis_cells(-5.0, 5.0, 0)
    src_y = axis_cells(-5.0, 5.0, 1)
    return {
        "schema_version": "openbnct.material-assignment/0.2.0",
        "case_id": "azmy-problem-full",
        "base_material": material("azmy.abs"),
        "regions": [
            {
                "name": "azmy.src",
                "material": material("azmy.src"),
                "shape": {
                    "kind": "voxel_box",
                    "lower": [src_x[0], src_y[0], 0],
                    "upper": [src_x[-1], src_y[-1], NZ - 1],
                },
            }
        ],
        "provenance_id": "azmy-1988-full-domain",
    }


def geometry():
    return {
        "shape": [NXY, NXY, NZ],
        "spacing_mm": SPACING_MM,
        "origin_mm": ORIGIN_MM,
        "direction": [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    }


def source():
    # q = 1 n/cm3/s over the source box -> total rate = V_box n/s.
    v = 10.0 * 10.0 * Z_CM
    return {
        "schema_version": "openbnct.fixed-source-definition/0.1.0",
        "id": "azmy-volume-source",
        "particle": "neutron",
        "source_sites_per_history": 1,
        "statistical_weight_per_site": 1.0 * v,
        "space": {
            "kind": "uniform_box",
            "x_range_cm": [-5.0, 5.0],
            "y_range_cm": [-5.0, 5.0],
            "z_range_cm": [-Z_CM / 2.0, Z_CM / 2.0],
            "interval_convention": "half_open",
        },
        "angle": {
            "kind": "isotropic_cone",
            "axis_unit_vector": [0.0, 0.0, 1.0],
            "half_angle_rad": 3.141592653589793,
        },
        "energy": {"kind": "monoenergetic", "energy_ev": 0.5},
    }


def case():
    return {
        "schema_version": "openbnct.transport-case/0.1.0",
        "case_id": "azmy-problem-full",
        "geometry": geometry(),
        "material": material("azmy.abs"),
        "source": source(),
        "requested_histories": 1,
    }


def main():
    for name, doc in {
        "multigroup-data.json": multigroup_data(),
        "assignment.json": assignment(),
        "case.json": case(),
    }.items():
        path = os.path.join(OUT, name)
        with open(path, "w") as fh:
            json.dump(doc, fh, indent=2)
            fh.write("\n")
        print("wrote", name)


if __name__ == "__main__":
    main()

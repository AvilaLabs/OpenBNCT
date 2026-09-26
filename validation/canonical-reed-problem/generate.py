#!/usr/bin/env python3
"""Generate the canonical Reed's-problem artifacts.

Deterministic generator: emits the transport cases, material
assignment, and one-group multigroup data for the mirror-domain
formulation of Reed's problem (Reed 1971; Warsa 2002 eigenfunction
reference in reference/warsa-2002-eigenfunction.csv).

Layout (x in cm, |x| the mirrored half):
    |x|<2   strong absorber+source   sigma_t=50, sigma_s=0,   q=50
    2..3    absorber                 sigma_t=5,  sigma_s=0,   q=0
    3..5    void                     sigma_t~0,  sigma_s=0,   q=0
    5..6    scattering source        sigma_t=1,  sigma_s=0.9, q=1
    6..8    reflector                sigma_t=1,  sigma_s=0.9, q=0
Mirror at x=0 (realized by mirroring the domain to [-8,8]); vacuum at
|x|=8. The second source occupies two disjoint slabs (5<|x|<6): linearity
lets one solve over [5,6] be superposed with its x->-x reflection.

Grid: 160 x-cells of 1 mm covering [-8,8] cm; 4x4 transverse cells of
1 mm over y,z in [-2,2] mm with periodic transverse boundaries (infinite
slab).
"""

import json
import os

OUT = os.environ.get("REED_OUT", os.path.dirname(os.path.abspath(__file__)))

# Grid constants: NX cells covering x in [-80,80] mm = [-8,8] cm;
# REED_NX overrides for the mesh-refinement study (spacing is
# 160/NX mm so the cell-center lattice stays commensurate with the
# material boundaries at |x| in {2,3,5,6}).
NX = int(os.environ.get("REED_NX", "160"))
NY, NZ = 4, 4
SPACING_MM = [160.0 / NX, 1.0, 1.0]
ORIGIN_MM = [-80.0 + SPACING_MM[0] / 2.0, -1.5, -1.5]
TRANSVERSE_CM = 4 * 0.1  # 0.4 cm in y and z


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
        "id": "reed-problem-1g",
        "energy_boundaries_ev": [1.0, 1.0e-3],
        "collapse_declaration": "Canonical one-group benchmark data "
        "(Reed 1971 NSE 46:309-314; Warsa 2002 ANE 29:851-874): "
        "declared macroscopic sigma_t / sigma_s0, isotropic scatter.",
        "component_profile": None,
        "materials": [
            mat("reed.src1", 50.0, 0.0),
            mat("reed.abs1", 5.0, 0.0),
            mat("reed.void", 1.0e-8, 0.0),
            mat("reed.src2", 1.0, 0.9),
            mat("reed.refl", 1.0, 0.9),
        ],
    }


def geometry():
    return {
        "shape": [NX, NY, NZ],
        "spacing_mm": SPACING_MM,
        "origin_mm": ORIGIN_MM,
        "direction": [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    }


def x_cells(x_lo_cm, x_hi_cm):
    """Cell indices whose centers lie in [x_lo, x_hi) cm."""
    out = []
    for i in range(NX):
        x_cm = (ORIGIN_MM[0] + SPACING_MM[0] * i) / 10.0
        if x_lo_cm - 1e-12 <= x_cm < x_hi_cm - 1e-12:
            out.append(i)
    return out


def vbox(x_lo, x_hi):
    return {
        "kind": "voxel_box",
        "lower": [x_lo, 0, 0],
        "upper": [x_hi, NY - 1, NZ - 1],
    }


def assignment():
    # Regions (mirrored): each covers the full transverse extent.
    # Symmetric x -> -x maps cell i -> 159-i.
    regions = []

    def add(name, mid, xlo, xhi):
        """|x| in [xlo, xhi): one box per contiguous mirrored run."""
        cells = sorted(set(x_cells(xlo, xhi)) | set(x_cells(-xhi, -xlo)))
        start = prev = cells[0]
        for c in cells[1:] + [None]:
            if c is None or c != prev + 1:
                side = "left" if start < NX // 2 else "right"
                regions.append(
                    {
                        "name": f"{name}.{side}",
                        "material": material(mid),
                        "shape": vbox(start, prev),
                    }
                )
                start = c
            if c is not None:
                prev = c

    add("reed.src1", "reed.src1", 0.0, 2.0)
    add("reed.abs1", "reed.abs1", 2.0, 3.0)
    add("reed.void", "reed.void", 3.0, 5.0)
    add("reed.src2", "reed.src2", 5.0, 6.0)
    # reflector is the base material — no region needed.
    return {
        "schema_version": "openbnct.material-assignment/0.2.0",
        "case_id": "reed-problem-mirror",
        "base_material": material("reed.refl"),
        "regions": regions,
        "provenance_id": "reed-1971-mirror-domain",
    }


def source_box(x_lo_cm, x_hi_cm, density_per_cm3):
    """UniformBox source; statistical weight = q * V_box (rate)."""
    v = (
        (x_hi_cm - x_lo_cm)
        * TRANSVERSE_CM
        * TRANSVERSE_CM
    )
    return {
        "schema_version": "openbnct.fixed-source-definition/0.1.0",
        "id": "reed-volume-source",
        "particle": "neutron",
        "source_sites_per_history": 1,
        "statistical_weight_per_site": density_per_cm3 * v,
        "space": {
            "kind": "uniform_box",
            "x_range_cm": [x_lo_cm, x_hi_cm],
            "y_range_cm": [-0.2, 0.2],
            "z_range_cm": [-0.2, 0.2],
            "interval_convention": "half_open",
        },
        "angle": {
            "kind": "isotropic_cone",
            "axis_unit_vector": [1.0, 0.0, 0.0],
            "half_angle_rad": 3.141592653589793,
        },
        "energy": {"kind": "monoenergetic", "energy_ev": 0.5},
    }


def case(case_id, source):
    return {
        "schema_version": "openbnct.transport-case/0.1.0",
        "case_id": case_id,
        "geometry": geometry(),
        "material": material("reed.refl"),
        "source": source,
        "requested_histories": 1,
    }


def main():
    artifacts = {
        "multigroup-data.json": multigroup_data(),
        "assignment.json": assignment(),
        # Solve A: the strong source, |x|<2 at q=50/cm3.
        "case-src1.json": case("reed-problem-mirror", source_box(-2.0, 2.0, 50.0)),
        # Solve B: one copy of the weak scattering source, x in [5,6]
        # at q=1/cm3; its mirror supplies the other copy.
        "case-src2.json": case("reed-problem-mirror", source_box(5.0, 6.0, 1.0)),
    }
    for name, doc in artifacts.items():
        path = os.path.join(OUT, name)
        with open(path, "w") as fh:
            json.dump(doc, fh, indent=2)
            fh.write("\n")
        print("wrote", name)


if __name__ == "__main__":
    main()

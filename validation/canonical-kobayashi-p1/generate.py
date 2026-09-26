#!/usr/bin/env python3
"""Generate the Kobayashi problem-1 (pure absorber) artifacts.

Kobayashi, Sugimura, Nagaya — "3-D Radiation Transport Benchmark
Problems and Results for Simple Geometries with Void Region," NEA/NSC
benchmark (1999 Madrid proceedings; PNE 39:119-144, 2001). Problem 1
is the nested-cubes ("box in a box") case on the quarter domain
x,y,z in [0,100] cm with reflecting x=y=z=0 planes:

    [0,10]^3           source   sigma_t = 0.1, sigma_s = 0 (case i), q = 1
    [0,50]^3 minus src void     sigma_t = 1e-4
    rest       shield   sigma_t = 0.1, sigma_s = 0

Vacuum on x,y,z = 100. Realized here on the mirrored full domain
[-100,100]^3 cm (the source cube becomes [-10,10]^3, void [-50,50]^3,
vacuum on all six faces) — reflection equivariance makes the quarter
octant identical to the quarter-domain solution.

Mesh: 2 cm cells (100^3). All material faces (+-10, +-50, +-100 cm)
and the ten published probe points (odd integer coordinates) sit
exactly on cell boundaries/centers.

Emit: multigroup-data.json, assignment.json, case.json.
"""

import json
import os

OUT = os.environ.get("KOB1_OUT", os.path.dirname(os.path.abspath(__file__)))

# Cells of 20 mm covering [-100,100] cm per axis.
CELL_MM = float(os.environ.get("KOB1_CELL_MM", "20.0"))
N = int(round(2000.0 / CELL_MM))
SPACING_MM = [CELL_MM] * 3
ORIGIN_MM = [-1000.0 + CELL_MM / 2.0] * 3


def material(mid):
    return {
        "schema_version": "openbnct.material-definition/0.1.0",
        "id": mid,
        "density_g_cm3": 1.0,
        "temperature_k": 294.0,
        "nuclides": [{"name": "X1", "mass_fraction": 1.0}],
        "neutron_thermal_treatment": "free_gas",
    }


def multigroup_data(scatter):
    """scatter=False -> case i (pure absorber); True -> case ii
    (sigma_s = 0.5 sigma_t in every region, including the void)."""
    rs = 0.5 if scatter else 0.0

    def mat(mid, sigma_t):
        return {
            "material_id": mid,
            "sigma_total_per_cm": [sigma_t],
            "scatter_matrix_per_cm": [rs * sigma_t],
            "dose_response_gy_cm2": {},
            "transport_mu_bar": [0.0],
        }

    tag = "ii" if scatter else "i"
    return {
        "schema_version": "openbnct.multigroup-data/0.1.0",
        "id": f"kobayashi-p1-{tag}-1g",
        "energy_boundaries_ev": [1.0, 1.0e-3],
        "collapse_declaration": "Canonical one-group benchmark data "
        "(Kobayashi et al. 2001 PNE 39:119-144, problem 1 case "
        f"{tag}): declared macroscopic sigma_t / sigma_s0.",
        "component_profile": None,
        "materials": [
            mat("koba.src", 0.1),
            mat("koba.void", 1.0e-4),
            mat("koba.shield", 0.1),
        ],
    }


def axis_cells(lo_cm, hi_cm):
    """Indices along one axis whose cell centers lie in [lo, hi) cm."""
    out = []
    for i in range(N):
        c_cm = (ORIGIN_MM[0] + SPACING_MM[0] * i) / 10.0
        if lo_cm - 1e-12 <= c_cm < hi_cm - 1e-12:
            out.append(i)
    return out


def box_region(name, mid, x0, x1, y0, y1, z0, z1):
    cx, cy, cz = (
        axis_cells(x0, x1),
        axis_cells(y0, y1),
        axis_cells(z0, z1),
    )
    return {
        "name": name,
        "material": material(mid),
        "shape": {
            "kind": "voxel_box",
            "lower": [cx[0], cy[0], cz[0]],
            "upper": [cx[-1], cy[-1], cz[-1]],
        },
    }


def assignment():
    # Region overlap is rejected — decompose the void shell
    # [-50,50]^3 \ [-10,10]^3 into the six disjoint slabs around the
    # source core.
    E, S = 50.0, 10.0
    slabs = [
        ("void.xm", -E, -S, -E, E, -E, E),
        ("void.xp", S, E, -E, E, -E, E),
        ("void.ym", -S, S, -E, -S, -E, E),
        ("void.yp", -S, S, S, E, -E, E),
        ("void.zm", -S, S, -S, S, -E, -S),
        ("void.zp", -S, S, -S, S, S, E),
    ]
    regions = [
        box_region(f"koba.{n}", "koba.void", *s) for n, *s in slabs
    ]
    regions.append(box_region("koba.src", "koba.src", -S, S, -S, S, -S, S))
    return {
        "schema_version": "openbnct.material-assignment/0.2.0",
        "case_id": "kobayashi-p1-mirror",
        "base_material": material("koba.shield"),
        "regions": regions,
        "provenance_id": "kobayashi-2001-p1-mirror-domain",
    }


def source():
    # q = 1 n/cm3/s over the mirrored [-10,10]^3 cm source cube.
    return {
        "schema_version": "openbnct.fixed-source-definition/0.1.0",
        "id": "kobayashi-p1-source",
        "particle": "neutron",
        "source_sites_per_history": 1,
        "statistical_weight_per_site": 8000.0,
        "space": {
            "kind": "uniform_box",
            "x_range_cm": [-10.0, 10.0],
            "y_range_cm": [-10.0, 10.0],
            "z_range_cm": [-10.0, 10.0],
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
        "case_id": "kobayashi-p1-mirror",
        "geometry": {
            "shape": [N, N, N],
            "spacing_mm": SPACING_MM,
            "origin_mm": ORIGIN_MM,
            "direction": [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        },
        "material": material("koba.shield"),
        "source": source(),
        "requested_histories": 1,
    }


def main():
    for name, doc in {
        "multigroup-data-i.json": multigroup_data(False),
        "multigroup-data-ii.json": multigroup_data(True),
        "assignment.json": assignment(),
        "case-i.json": case(),
        "case-ii.json": case(),
    }.items():
        path = os.path.join(OUT, name)
        with open(path, "w") as fh:
            json.dump(doc, fh, indent=2)
            fh.write("\n")
        print("wrote", name)


if __name__ == "__main__":
    main()

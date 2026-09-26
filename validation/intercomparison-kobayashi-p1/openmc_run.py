#!/usr/bin/env python3
"""OpenMC multi-group realization of Kobayashi problem 1, case ii.

Same mirrored geometry as the deterministic case (see
../canonical-kobayashi-p1/): domain [-100,100]^3 cm vacuum-bounded,
source cube [-10,10]^3 at q = 1 n/cm3/s (8000 n/s), void shell
[-50,50]^3 minus source, absorber-scatterer shield sigma_t=0.1,
sigma_s0=0.05; void sigma_t=1e-4, sigma_s0=5e-5. One group,
isotropic (P0) scattering.

Tallies flux on the same 2 cm cell lattice as the S_N solve so the
comparison is cell-mean vs cell-mean. Probe extraction mirrors
compare_ii.py.
"""

import json
import os
import sys

import numpy as np
import openmc

HERE = os.path.dirname(os.path.abspath(__file__))
CELL_CM = float(os.environ.get("KOB1_CELL_MM", "20.0")) / 10.0
N = int(round(200.0 / CELL_CM))
X0 = -100.0 + CELL_CM / 2.0
BATCHES = int(os.environ.get("KOB1_MC_BATCHES", "200"))
PARTICLES = int(os.environ.get("KOB1_MC_PARTICLES", "100000"))

PROBES = [(5, y, 5) for y in (5, 15, 25, 35, 45, 55, 65, 75, 85, 95)] + [
    (d, d, d) for d in (15, 25, 35, 45, 55)
]


def build_library(path):
    groups = openmc.mgxs.EnergyGroups([1.0e-3, 1.0])
    lib = openmc.MGXSLibrary(groups)
    for name, sig_t, sig_a in (
        ("shield", 0.1, 0.05),
        ("void", 1.0e-4, 5.0e-5),
    ):
        xs = openmc.XSdata(name, groups)
        xs.order = 0
        xs.set_total([sig_t])
        xs.set_absorption([sig_a])
        xs.set_scatter_matrix([[[sig_t - sig_a]]])
        lib.add_xsdata(xs)
    os.makedirs(path, exist_ok=True)
    lib.export_to_hdf5(os.path.join(path, "mgxs.h5"))
    return os.path.join(path, "mgxs.h5")


def build_model(xs_path):
    shield = openmc.Material(name="shield")
    shield.add_macroscopic("shield")
    void = openmc.Material(name="void")
    void.add_macroscopic("void")

    s = openmc.model.RectangularParallelepiped(-10, 10, -10, 10, -10, 10)
    v = openmc.model.RectangularParallelepiped(-50, 50, -50, 50, -50, 50)
    d = openmc.model.RectangularParallelepiped(
        -100, 100, -100, 100, -100, 100, boundary_type="vacuum"
    )
    cells = [
        openmc.Cell(name="src", fill=shield, region=-s),
        openmc.Cell(name="void", fill=void, region=+s & -v),
        openmc.Cell(name="shield", fill=shield, region=+v & -d),
    ]
    geom = openmc.Geometry(cells)

    box = openmc.stats.Box([-9.9999999, -9.9999999, -9.9999999], [9.9999999, 9.9999999, 9.9999999])
    # Source strength 8000 n/s; point tallies are per-source-particle,
    # and OpenMC normalizes flux tallies by source strength — the GMVP
    # table is in absolute n/cm2/s for q=1, so scale by 8000.
    src = openmc.IndependentSource(space=box, strength=8000.0,
        particle="neutron", domains=[cells[0]],
        energy=openmc.stats.Discrete([0.5], [1.0]))

    settings = openmc.Settings()
    settings.energy_mode = "multi-group"
    settings.source = src
    settings.batches = BATCHES
    settings.inactive = 0
    settings.particles = PARTICLES
    settings.run_mode = "fixed source"
    settings.source_rejection_fraction = 0.01

    mesh = openmc.RegularMesh()
    mesh.lower_left = [-100.0] * 3
    mesh.upper_right = [100.0] * 3
    mesh.dimension = [N, N, N]
    tally = openmc.Tally(name="flux")
    tally.filters = [openmc.MeshFilter(mesh)]
    tally.scores = ["flux"]
    tallies = openmc.Tallies([tally])

    model = openmc.Model(geom, None, settings, tallies)
    model.materials.cross_sections = xs_path
    return model, mesh


def main():
    out_dir = os.path.join(HERE, "openmc_run")
    xs_path = build_library(os.path.join(out_dir, "xsdata"))
    model, _mesh = build_model(xs_path)
    model.materials.cross_sections = xs_path
    sp_path = model.run(cwd=out_dir)

    with openmc.StatePoint(sp_path) as sp:
        tally = sp.get_tally(name="flux")
    mean = tally.get_reshaped_data("mean").flatten()  # per mesh cell
    stddev = tally.get_reshaped_data("std_dev").flatten()

    def idx(x, y, z):
        return (
            int(round((x - X0) / CELL_CM))
            + N * int(round((y - X0) / CELL_CM))
            + N * N * int(round((z - X0) / CELL_CM))
        )

    probes = []
    for pt in PROBES:
        i = idx(*[float(c) for c in pt])
        # flux tally units: particle-cm per source particle; divide by
        # cell volume for the flux-per-source normalization OpenMC uses.
        vol = CELL_CM**3
        probes.append(
            {
                "point_cm": pt,
                "phi_mc": mean[i] / vol * 8000.0,
                "phi_mc_std": stddev[i] / vol * 8000.0,
            }
        )
        print(
            f"{pt}: phi={probes[-1]['phi_mc']:.4e} "
            f"+/- {probes[-1]['phi_mc_std']:.2e}"
        )
    with open(os.path.join(HERE, "openmc-flux.json"), "w") as fh:
        json.dump(
            {
                "schema_version": "openbnct.validation-comparison/0.1.0",
                "case": "intercomparison-kobayashi-p1-ii",
                "engine": f"openmc {openmc.__version__} multi-group",
                "batches": BATCHES,
                "particles_per_batch": PARTICLES,
                "probes": probes,
            },
            fh,
            indent=2,
        )
        fh.write("\n")


if __name__ == "__main__":
    sys.exit(main())

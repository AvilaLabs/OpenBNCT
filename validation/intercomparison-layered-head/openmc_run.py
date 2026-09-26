#!/usr/bin/env python3
"""Continuous-energy OpenMC realization of the layered-head phantom
beam case — the MC leg of the S_N intercomparison.

Geometry/materials are read directly from the shared benchmark
artifacts (benchmarks/synthetic/layered-head-phantom/assignment.json
+ materials/*.json) so both codes see the identical model. The beam
matches case.json: uniform disk r=7cm at the z=0 face, isotropic
cone half-angle 0.1491 rad about +z, three-bin tabulated spectrum.

Tallies (on the native 25x25x25 x 8mm voxel lattice):
  * flux x 5 energy bins [1e-5, 0.1, 0.5, 4e3, 2e5, 2e7] eV —
    resolves the subthermal population whose 28g->56g condensation
    is the open question
  * absorption rate on B10 and N14 (the capture-dose channels)
"""

import json
import os
import sys

import numpy as np
import openmc

HERE = os.path.dirname(os.path.abspath(__file__))
BENCH = os.path.join(
    HERE, "..", "..", "benchmarks", "synthetic", "layered-head-phantom"
)
OUT = os.path.join(HERE, "openmc_run")
BATCHES = int(os.environ.get("LHP_MC_BATCHES", "20"))
PARTICLES = int(os.environ.get("LHP_MC_PARTICLES", "20000"))

EBINS = [1.0e-5, 0.1, 0.5, 4.0e3, 2.0e5, 2.0e7]  # eV


def load_phantom():
    assign = json.load(open(os.path.join(BENCH, "assignment.json")))
    mats = {}
    for name in ("skin", "skull", "brain", "void"):
        mats[name] = json.load(
            open(os.path.join(BENCH, "materials", f"{name}.json"))
        )
    return assign, mats


def build():
    assign, defs = load_phantom()

    om_mats = {}
    for name, m in defs.items():
        om = openmc.Material(name=name)
        for n in m["nuclides"]:
            om.add_nuclide(n["name"], n["mass_fraction"], "wo")
        om.set_density("g/cm3", m["density_g_cm3"])
        om_mats[name] = om
    # Near-void base: OpenMC has no rho=1e-9 transport; use void.
    void_u = openmc.Universe(name="void")
    void_u.add_cell(openmc.Cell())
    mat_universe = {}
    for name, om in om_mats.items():
        if name == "void":
            continue
        u = openmc.Universe(name=name)
        u.add_cell(openmc.Cell(fill=om))
        mat_universe[name] = u

    # Voxel index -> material name from the assignment regions.
    shape = (25, 25, 25)
    mat_idx = np.full(shape, "void", dtype=object)
    name_by_id = {
        "openbnct.layered-head.skin.v1": "skin",
        "openbnct.layered-head.skull.v1": "skull",
        "openbnct.layered-head.brain.v1": "brain",
    }
    for region in assign["regions"]:
        mid = name_by_id[region["material"]["id"]]
        if region["shape"]["kind"] == "voxel_set":
            for i, j, k in region["shape"]["indices"]:
                mat_idx[i, j, k] = mid
        else:
            lo, hi = region["shape"]["lower"], region["shape"]["upper"]
            for i in range(lo[0], hi[0] + 1):
                for j in range(lo[1], hi[1] + 1):
                    for k in range(lo[2], hi[2] + 1):
                        mat_idx[i, j, k] = mid

    # RectLattice.universes is indexed [iz][iy][ix] with iy counted
    # top-down: lattice row 0 is the highest-y element.
    lat = openmc.RectLattice(name="phantom")
    lat.lower_left = (-10.0, -10.0, 0.0)
    lat.pitch = (0.8, 0.8, 0.8)
    lat.universes = np.array(
        [
            [
                [
                    mat_universe.get(mat_idx[i, 24 - j, k], void_u)
                    for i in range(25)
                ]
                for j in range(25)
            ]
            for k in range(25)
        ]
    )

    domain = openmc.model.RectangularParallelepiped(
        -10, 10, -10, 10, 0, 20, boundary_type="vacuum"
    )
    root = openmc.Cell(name="root", fill=lat, region=-domain)
    geom = openmc.Geometry([root])

    disk = openmc.stats.CylindricalIndependent(
        r=openmc.stats.PowerLaw(0.0, 7.0, 1),
        phi=openmc.stats.Uniform(0.0, 2 * np.pi),
        z=openmc.stats.Discrete([0.0], [1.0]),
    )
    cone = openmc.stats.PolarAzimuthal(
        mu=openmc.stats.Uniform(np.cos(0.1491), 1.0),
        phi=openmc.stats.Uniform(0.0, 2 * np.pi),
        reference_uvw=(0.0, 0.0, 1.0),
    )
    # Tabular(histogram) expects pdf HEIGHTS — bin probability w_i
    # converts to height w_i/(E_hi-E_lo).
    bounds = [1e-5, 0.5, 1e4, 1.69e7]
    probs = [0.0611, 0.9093, 0.0296]
    spectrum = openmc.stats.Tabular(
        bounds,
        [w / (bounds[i + 1] - bounds[i]) for i, w in enumerate(probs)],
        interpolation="histogram",
    )
    src = openmc.IndependentSource(
        space=disk, angle=cone, energy=spectrum, particle="neutron"
    )

    settings = openmc.Settings()
    settings.source = src
    settings.batches = BATCHES
    settings.inactive = 0
    settings.particles = PARTICLES
    settings.run_mode = "fixed source"

    mesh = openmc.RegularMesh()
    mesh.lower_left = (-10.0, -10.0, 0.0)
    mesh.upper_right = (10.0, 10.0, 20.0)
    mesh.dimension = [25, 25, 25]
    efilt = openmc.EnergyFilter(EBINS)
    mfilt = openmc.MeshFilter(mesh)

    t_flux = openmc.Tally(name="flux-egroup")
    t_flux.filters = [mfilt, efilt]
    t_flux.scores = ["flux"]

    t_b10 = openmc.Tally(name="abs-b10")
    t_b10.filters = [mfilt]
    t_b10.nuclides = ["B10"]
    t_b10.scores = ["absorption"]

    t_n14 = openmc.Tally(name="abs-n14")
    t_n14.filters = [mfilt]
    t_n14.nuclides = ["N14"]
    t_n14.scores = ["absorption"]

    materials = openmc.Materials(geom.get_all_materials().values())
    return openmc.Model(
        geom, materials, settings, openmc.Tallies([t_flux, t_b10, t_n14])
    )


def extract(sp_path):
    with openmc.StatePoint(sp_path) as sp:
        tf = sp.get_tally(name="flux-egroup")
        tb = sp.get_tally(name="abs-b10")
        tn = sp.get_tally(name="abs-n14")
    fmean = tf.get_reshaped_data("mean").flatten()
    fstd = tf.get_reshaped_data("std_dev").flatten()
    bmean = tb.get_reshaped_data("mean").flatten()
    bstd = tb.get_reshaped_data("std_dev").flatten()
    nmean = tn.get_reshaped_data("mean").flatten()
    nstd = tn.get_reshaped_data("std_dev").flatten()
    vol = 0.8**3
    out = {
        "schema_version": "openbnct.validation-comparison/0.1.0",
        "case": "intercomparison-layered-head",
        "engine": f"openmc {openmc.__version__} continuous-energy endfb-viii.1",
        "batches": BATCHES,
        "particles_per_batch": PARTICLES,
        "energy_bins_ev": EBINS,
        "cell_vol_cm3": vol,
        "flux_mean": (fmean / vol).tolist(),
        "flux_std": (fstd / vol).tolist(),
        "abs_b10_mean": (bmean / vol).tolist(),
        "abs_b10_std": (bstd / vol).tolist(),
        "abs_n14_mean": (nmean / vol).tolist(),
        "abs_n14_std": (nstd / vol).tolist(),
    }
    path = os.path.join(HERE, "openmc-tallies.json")
    json.dump(out, open(path, "w"))
    return out


def main():
    model = build()
    sp = model.run(cwd=OUT)
    out = extract(sp)
    f = np.array(out["flux_mean"]).reshape(-1, 5)
    tot = f.sum(1)
    tot = tot[tot > 0]
    print(
        f"voxels tallied>0: {len(tot)}, sub-0.5eV fraction median "
        f"{np.median(f[f[:,0]>0][:,0]/f.sum(1)[f[:,0]>0]):.3f}"
    )
    print("wrote", os.path.join(HERE, "openmc-tallies.json"))


if __name__ == "__main__":
    sys.exit(main())

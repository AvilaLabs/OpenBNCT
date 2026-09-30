#!/usr/bin/env python3
"""Case B: 1-D brain column (1x1x25 cells of 8 mm), monodirectional +z beam
on the z = 0 face, transverse faces reflective (S_N: periodic x,y),
vacuum front/back.

Writes reference.json, sn-case.json and data-identity.json.
Run:  OMP_NUM_THREADS=3 ~/micromamba/envs/openmc016/bin/python generate_reference.py
"""

import json
import math
import os
import sys

import numpy as np
import openmc

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import common  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
TAG = "column-1d-brain"
NZ = 25
DX_CM = 0.8
LENGTH = NZ * DX_CM  # 20 cm
DISK_RADIUS_CM = 0.6  # S_N face source; covers the one cell by centre
PARTICLES = 200_000
BATCHES = 10  # 2e6 histories
SEED = 20260930


def sn_case(brain_def):
    return {
        "schema_version": "openbnct.transport-case/0.1.0",
        "case_id": "openbnct.regression.column-1d-brain.v1",
        "geometry": {
            "shape": [1, 1, NZ],
            "spacing_mm": [DX_CM * 10.0] * 3,
            "origin_mm": [0.0, 0.0, DX_CM * 5.0],  # cell-0 CENTRE; low face at z = 0
            "direction": [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        },
        "material": brain_def,
        "source": {
            "schema_version": "openbnct.fixed-source-definition/0.1.0",
            "id": "openbnct.regression.column-beam",
            "particle": "neutron",
            "source_sites_per_history": 1,
            "statistical_weight_per_site": 1.0,
            "space": {
                "kind": "uniform_disk",
                "axis": "z",
                "offset_cm": 0.0,
                "center_uv_cm": [0.0, 0.0],
                "radius_cm": DISK_RADIUS_CM,
            },
            "angle": {"kind": "monodirectional", "unit_vector": [0.0, 0.0, 1.0]},
            "energy": {
                "kind": "tabulated_histogram",
                "energy_boundaries_ev": common.SRC_BOUNDS_EV,
                "bin_weights": common.SRC_PROBS,
            },
        },
        "requested_histories": 1,
    }


def main():
    common.check_spectrum_normalization()
    work = common.workdir(TAG)
    xs = common.build_cross_sections(work)
    brain, brain_def = common.load_material("brain")

    hw = DX_CM / 2
    box = openmc.model.RectangularParallelepiped(-hw, hw, -hw, hw, 0, LENGTH)
    box.xmin.boundary_type = "reflective"
    box.xmax.boundary_type = "reflective"
    box.ymin.boundary_type = "reflective"
    box.ymax.boundary_type = "reflective"
    box.zmin.boundary_type = "vacuum"
    box.zmax.boundary_type = "vacuum"
    cell = openmc.Cell(fill=brain, region=-box)
    geom = openmc.Geometry([cell])

    # Uniform over the one cell face (0.8 x 0.8 cm), +z, entering at z = 1e-8 cm.
    src = openmc.IndependentSource(
        space=openmc.stats.CartesianIndependent(
            openmc.stats.Uniform(-hw, hw),
            openmc.stats.Uniform(-hw, hw),
            openmc.stats.Discrete([1.0e-8], [1.0]),
        ),
        angle=openmc.stats.Monodirectional((0.0, 0.0, 1.0)),
        energy=common.source_spectrum(),
        particle="neutron",
    )
    st = common.settings(PARTICLES, BATCHES, SEED, xs)
    st.source = src

    bounds = common.sn_group_bounds_ascending()
    mesh = openmc.RegularMesh()
    mesh.lower_left = (-hw, -hw, 0.0)
    mesh.upper_right = (hw, hw, LENGTH)
    mesh.dimension = (1, 1, NZ)
    t_flux = openmc.Tally(name="flux")
    t_flux.filters = [openmc.MeshFilter(mesh), openmc.EnergyFilter(bounds)]
    t_flux.scores = ["flux"]
    t_abs = openmc.Tally(name="absorption")
    t_abs.scores = ["absorption"]
    t_abs.estimator = "analog"
    t_leak = openmc.Tally(name="leak")
    t_leak.filters = [openmc.SurfaceFilter([box.zmin, box.zmax])]
    t_leak.scores = ["current"]
    model = openmc.Model(
        geom, common.materials([brain], xs), st, openmc.Tallies([t_flux, t_abs, t_leak])
    )
    sp_path = common.run_model(model, work)

    with openmc.StatePoint(sp_path) as sp:
        tf = sp.get_tally(name="flux")
        ta = sp.get_tally(name="absorption")
        tl = sp.get_tally(name="leak")
        fm = tf.get_reshaped_data("mean").reshape(NZ, -1)  # [voxel, group ascending]
        fs = tf.get_reshaped_data("std_dev").reshape(NZ, -1)
        am, asd = float(ta.mean.flatten()[0]), float(ta.std_dev.flatten()[0])
        leak = tl.mean.flatten()

    # Track length [cm] / axial thickness [cm] = flux per unit AREAL source
    # density (n/cm2 on the face); comparable to S_N flux * (pi r^2). README.
    fm = fm / DX_CM
    fs = fs / DX_CM

    # Sanity 1: every source neutron is absorbed or leaks (front + back current).
    # Front-face current is scored with sign; the two surfaces' |net| plus
    # absorption must equal 1 per source neutron (no multiplication in this tissue
    # except small (n,2n); allow 0.5 %).
    total = am + abs(leak[0]) + abs(leak[1])
    assert abs(total - 1.0) < 5e-3, f"absorbed+leaked = {total} (abs {am}, leak {leak})"
    # Sanity 2: entrance-voxel fast flux vs the analytic uncollided flux.
    # Per unit areal density a +z beam has flux 1 (mu = 1); the uncollided flux
    # averaged over voxel 0 is <(1 - exp(-St dz)) / (St dz)>, averaged over the
    # uniform-per-eV fast bin. The MC value (per fast source neutron) must lie
    # above it (scattered neutrons add track length, ~1/mu) but not wildly so.
    bounds_a = np.array(bounds)
    bands = np.array(common.band_of_group_ascending(bounds))
    fast0 = fm[0][bands == 2].sum() / common.SRC_PROBS[2]
    e = np.linspace(common.EPITHERMAL_MAX_EV, common.SRC_BOUNDS_EV[3], 40001)
    tau = common.macroscopic_total_xs(brain, e) * DX_CM
    unc0 = float(np.mean((1 - np.exp(-tau)) / tau))
    assert 1.0 * unc0 <= fast0 <= 1.6 * unc0, f"entrance fast {fast0} vs uncollided {unc0}"

    band_flux = np.stack([fm[:, bands == b].sum(axis=1) for b in range(3)], axis=1)
    band_std = np.stack(
        [np.sqrt((fs[:, bands == b] ** 2).sum(axis=1)) for b in range(3)], axis=1
    )
    ref = {
        "schema_version": "openbnct.regression-reference/0.1.0",
        "case": TAG,
        "engine": f"openmc {openmc.__version__} continuous-energy endfb-viii.1 + c_H_in_H2O",
        "histories": PARTICLES * BATCHES,
        "seed": SEED,
        "normalization": "per source neutron, uniform over the one 0.8x0.8 cm cell face; "
        "flux = track length in voxel / axial thickness = flux per unit areal source "
        "density [n/cm2]. S_N (disk r=0.6 cm, total 1) flux times pi*r^2 is comparable.",
        "disk_area_cm2": math.pi * DISK_RADIUS_CM**2,
        "voxel_thickness_cm": DX_CM,
        "group_bounds_ev_ascending": bounds,
        "flux_mean_voxel_x_group_ascending": common.fmt(fm),
        "flux_std_voxel_x_group_ascending": common.fmt(fs),
        "band_flux_mean_voxel_x_band": common.fmt(band_flux),
        "band_flux_std_voxel_x_band": common.fmt(band_std),
        "band_order": ["thermal", "epithermal", "fast"],
        "band_definition_ev": {
            "thermal": [bounds_a[0], common.THERMAL_MAX_EV],
            "epithermal": [common.THERMAL_MAX_EV, common.EPITHERMAL_MAX_EV],
            "fast": [common.EPITHERMAL_MAX_EV, bounds_a[-1]],
        },
        "absorption_mean": am,
        "absorption_std": asd,
        "leakage_front_back": [float(leak[0]), float(leak[1])],
        "voxels": NZ,
    }
    with open(os.path.join(HERE, "reference.json"), "w") as f:
        json.dump(ref, f, indent=1)
        f.write("\n")
    with open(os.path.join(HERE, "sn-case.json"), "w") as f:
        json.dump(sn_case(brain_def), f, indent=1)
        f.write("\n")
    common.write_identity(
        HERE,
        TAG,
        xs,
        {
            "particles_per_batch": PARTICLES,
            "batches": BATCHES,
            "total_histories": PARTICLES * BATCHES,
            "seed": SEED,
            "materials": ["benchmarks/synthetic/layered-head-phantom/materials/brain.json"],
        },
    )
    print("absorption", am, asd, "leak", leak, "fast0", fast0, "uncollided", unc0)
    print("band totals", band_flux.sum(axis=0))


if __name__ == "__main__":
    main()

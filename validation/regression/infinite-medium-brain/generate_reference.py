#!/usr/bin/env python3
"""Case A: infinite brain medium, uniform volumetric source.

Writes reference.json (continuous-energy OpenMC group flux + absorption per
source neutron), sn-case.json (the matching S_N case) and data-identity.json.
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
TAG = "infinite-medium-brain"
N = 4  # cells per side
DX_CM = 2.0
SIDE = N * DX_CM  # 8 cm cube
PARTICLES = 100_000
BATCHES = 10  # 1e6 histories
SEED = 20260929


def sn_case(brain_def):
    volume = SIDE**3
    return {
        "schema_version": "openbnct.transport-case/0.1.0",
        "case_id": "openbnct.regression.infinite-medium-brain.v1",
        "geometry": {
            "shape": [N, N, N],
            "spacing_mm": [DX_CM * 10.0] * 3,
            "origin_mm": [0.0, 0.0, 0.0],  # cell-0 CENTER
            "direction": [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        },
        "material": brain_def,
        "source": {
            "schema_version": "openbnct.fixed-source-definition/0.1.0",
            "id": "openbnct.regression.volumetric-q1",
            "particle": "neutron",
            "source_sites_per_history": 1,
            # UniformBox: weight is the total emission rate; density = weight / V = 1 /cm3.
            "statistical_weight_per_site": volume,
            "space": {
                "kind": "uniform_box",
                "x_range_cm": [-DX_CM / 2, SIDE - DX_CM / 2],
                "y_range_cm": [-DX_CM / 2, SIDE - DX_CM / 2],
                "z_range_cm": [-DX_CM / 2, SIDE - DX_CM / 2],
                "interval_convention": "half_open",
            },
            "angle": {
                "kind": "isotropic_cone",
                "axis_unit_vector": [0.0, 0.0, 1.0],
                "half_angle_rad": math.pi,
            },
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

    box = openmc.model.RectangularParallelepiped(
        0, SIDE, 0, SIDE, 0, SIDE, boundary_type="reflective"
    )
    cell = openmc.Cell(fill=brain, region=-box)
    geom = openmc.Geometry([cell])

    src = openmc.IndependentSource(
        space=openmc.stats.Box((0, 0, 0), (SIDE, SIDE, SIDE)),
        angle=openmc.stats.Isotropic(),
        energy=common.source_spectrum(),
        particle="neutron",
    )
    st = common.settings(PARTICLES, BATCHES, SEED, xs)
    st.source = src

    bounds = common.sn_group_bounds_ascending()
    efilt = openmc.EnergyFilter(bounds)
    cfilt = openmc.CellFilter(cell)
    t_flux = openmc.Tally(name="flux")
    t_flux.filters = [cfilt, efilt]
    t_flux.scores = ["flux"]
    t_abs = openmc.Tally(name="absorption")  # analog: exact conservation count
    t_abs.filters = [cfilt]
    t_abs.scores = ["absorption"]
    t_abs.estimator = "analog"
    model = openmc.Model(
        geom, common.materials([brain], xs), st, openmc.Tallies([t_flux, t_abs])
    )
    sp_path = common.run_model(model, work)

    with openmc.StatePoint(sp_path) as sp:
        tf = sp.get_tally(name="flux")
        ta = sp.get_tally(name="absorption")
        # Track-length flux, cm per source neutron, integrated over the cube:
        # NOT divided by V (see README).
        fm = tf.get_reshaped_data("mean").flatten()
        fs = tf.get_reshaped_data("std_dev").flatten()
        am = float(ta.mean.flatten()[0])
        asd = float(ta.std_dev.flatten()[0])

    # Sanity 1: infinite-medium absorption per source neutron is 1.
    assert abs(am - 1.0) < max(5 * asd, 2e-3), f"absorption {am} +- {asd}"
    # Sanity 2: track-length flux weighted by the group-collapsed Sigma_a
    # is not available here; instead check the leak-free balance above.

    bands = np.array(common.band_of_group_ascending(bounds))
    band_mean = [float(fm[bands == b].sum()) for b in range(3)]
    band_std = [float(np.sqrt((fs[bands == b] ** 2).sum())) for b in range(3)]

    ref = {
        "schema_version": "openbnct.regression-reference/0.1.0",
        "case": TAG,
        "engine": f"openmc {openmc.__version__} continuous-energy endfb-viii.1 + c_H_in_H2O",
        "histories": PARTICLES * BATCHES,
        "seed": SEED,
        "normalization": "per source neutron; flux = track length [cm] summed over the "
        "whole cube, NOT divided by V (equals S_N flux for q = 1 /cm3)",
        "group_bounds_ev_ascending": bounds,
        "flux_mean_ascending": common.fmt(fm),
        "flux_std_ascending": common.fmt(fs),
        "band_definition_ev": {
            "thermal": [bounds[0], common.THERMAL_MAX_EV],
            "epithermal": [common.THERMAL_MAX_EV, common.EPITHERMAL_MAX_EV],
            "fast": [common.EPITHERMAL_MAX_EV, bounds[-1]],
        },
        "band_flux_mean": common.fmt(band_mean),
        "band_flux_std": common.fmt(band_std),
        "absorption_mean": am,
        "absorption_std": asd,
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
    print("bands (thermal, epithermal, fast):", band_mean, band_std)
    print("absorption:", am, asd)


if __name__ == "__main__":
    main()

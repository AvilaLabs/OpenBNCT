#!/usr/bin/env python3
"""Case C: mini layered head (9 x 9 x 9 voxels of 24 mm) derived from the
25^3 x 8 mm layered-head-phantom assignment by 3x3x3 majority vote.

Writes reference.json, sn-case.json, sn-assignment.json and data-identity.json.
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
TAG = "mini-layered-head"
N = 9
BLOCK = 3
DX_CM = 2.4
# Void padding (low, high) in fine cells per axis so that 25 + pad = N * BLOCK = 27:
# x, y symmetric (the beam axis stays a block centre); z high side only (the
# beam-entrance face z = 0 stays on the grid face).
PAD = [(1, 1), (1, 1), (0, 2)]
PARTICLES = 100_000
BATCHES = 20  # 2e6 histories
SEED = 20260931

MAT_NAMES = {1: "skin", 2: "skull", 3: "brain"}
# Tie-break for the majority vote: keep the thin skull, then skin, then brain, then void.
TIE_PRIORITY = [2, 1, 3, 0]


def fine_grid():
    """25^3 material-index grid (0 void, 1 skin, 2 skull, 3 brain), [i, j, k]."""
    assign = json.load(open(os.path.join(common.BENCH, "assignment.json")))
    ids = {
        "openbnct.layered-head.skin.v1": 1,
        "openbnct.layered-head.skull.v1": 2,
        "openbnct.layered-head.brain.v1": 3,
    }
    g = np.zeros((25, 25, 25), dtype=int)
    for region in assign["regions"]:
        m = ids[region["material"]["id"]]
        assert region["shape"]["kind"] == "voxel_set"
        for i, j, k in region["shape"]["indices"]:
            g[i, j, k] = m
    return assign, g


def downsample(g):
    """3x3x3 majority vote after void padding (PAD). Ties go to TIE_PRIORITY."""
    p = np.pad(g, PAD, constant_values=0)
    assert p.shape == (N * BLOCK,) * 3
    out = np.zeros((N, N, N), dtype=int)
    for i in range(N):
        for j in range(N):
            for k in range(N):
                b = p[BLOCK * i : BLOCK * i + BLOCK, BLOCK * j : BLOCK * j + BLOCK,
                      BLOCK * k : BLOCK * k + BLOCK].ravel()
                counts = np.bincount(b, minlength=4)
                best = max(counts)
                out[i, j, k] = next(m for m in TIE_PRIORITY if counts[m] == best)
    return out


def geometry():
    # Fine grid: cell-0 centres at (-96, -96, 4) mm, pitch 8. Block 0 spans fine
    # cells (-1..1) in x, y (centre = fine cell 0 = -96 mm) and (0..2) in z
    # (centre = fine cell 1 = 12 mm), so the low z face is at 0 and the beam axis
    # (x = y = 0) is the centre of block 4.
    return {
        "shape": [N, N, N],
        "spacing_mm": [DX_CM * 10.0] * 3,
        "origin_mm": [-96.0, -96.0, 12.0],
        "direction": [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    }


def sn_files(assign, grid):
    case = json.load(open(os.path.join(common.BENCH, "case.json")))
    case["case_id"] = "openbnct.regression.mini-layered-head.v1"
    case["geometry"] = geometry()
    regions = []
    mats = {r["material"]["id"]: r["material"] for r in assign["regions"]}
    by_index = {1: "skin", 2: "skull", 3: "brain"}
    for m, nm in by_index.items():
        idx = [
            [int(i), int(j), int(k)] for k in range(N) for j in range(N) for i in range(N)
            if grid[i, j, k] == m
        ]
        regions.append(
            {
                "name": f"{nm}-mini",
                "material": mats[f"openbnct.layered-head.{nm}.v1"],
                "shape": {"kind": "voxel_set", "indices": idx},
            }
        )
    mini = {
        "schema_version": assign["schema_version"],
        "case_id": case["case_id"],
        "base_material": assign["base_material"],
        "regions": regions,
        "provenance_id": "regression-mini-layered-head-2x2x2-majority",
    }
    return case, mini


def build_model(grid, xs, work):
    mats = {}
    mat_universe = {}
    for m, nm in MAT_NAMES.items():
        om, _ = common.load_material(nm)
        mats[m] = om
        u = openmc.Universe(name=nm)
        u.add_cell(openmc.Cell(fill=om))
        mat_universe[m] = u
    void_u = openmc.Universe(name="void")
    void_u.add_cell(openmc.Cell())

    pitch = DX_CM
    lo = (-10.8, -10.8, 0.0)
    hi = tuple(l + N * pitch for l in lo)
    lat = openmc.RectLattice(name="mini-head")
    lat.lower_left = lo
    lat.pitch = (pitch, pitch, pitch)
    # RectLattice.universes is indexed [iz][iy][ix] with iy counted top-down.
    lat.universes = np.array(
        [
            [[mat_universe.get(int(grid[i, N - 1 - j, k]), void_u) for i in range(N)]
             for j in range(N)]
            for k in range(N)
        ]
    )
    dom = openmc.model.RectangularParallelepiped(
        lo[0], hi[0], lo[1], hi[1], lo[2], hi[2], boundary_type="vacuum"
    )
    geom = openmc.Geometry([openmc.Cell(name="root", fill=lat, region=-dom)])

    src = openmc.IndependentSource(
        space=openmc.stats.CylindricalIndependent(
            r=openmc.stats.PowerLaw(0.0, 7.0, 1),
            phi=openmc.stats.Uniform(0.0, 2 * math.pi),
            z=openmc.stats.Discrete([1.0e-8], [1.0]),
        ),
        angle=openmc.stats.PolarAzimuthal(
            mu=openmc.stats.Uniform(math.cos(0.1491), 1.0),
            phi=openmc.stats.Uniform(0.0, 2 * math.pi),
            reference_uvw=(0.0, 0.0, 1.0),
        ),
        energy=common.source_spectrum(),
        particle="neutron",
    )
    st = common.settings(PARTICLES, BATCHES, SEED, xs)
    st.source = src

    mesh = openmc.RegularMesh()
    mesh.lower_left = lo
    mesh.upper_right = hi
    mesh.dimension = (N, N, N)
    mf = openmc.MeshFilter(mesh)
    band_edges = [common.SRC_BOUNDS_EV[0], common.THERMAL_MAX_EV, common.EPITHERMAL_MAX_EV,
                  common.SRC_BOUNDS_EV[3]]
    t_flux = openmc.Tally(name="flux-bands")
    t_flux.filters = [mf, openmc.EnergyFilter(band_edges)]
    t_flux.scores = ["flux"]
    t_b = openmc.Tally(name="abs-b10")
    t_b.filters = [mf]
    t_b.nuclides = ["B10"]
    t_b.scores = ["absorption"]
    t_n = openmc.Tally(name="abs-n14")
    t_n.filters = [mf]
    t_n.nuclides = ["N14"]
    t_n.scores = ["absorption"]
    all_mats = [mats[m] for m in sorted(mats)]
    return openmc.Model(
        geom, common.materials(all_mats, xs), st, openmc.Tallies([t_flux, t_b, t_n])
    )


def main():
    common.check_spectrum_normalization()
    assign, fine = fine_grid()
    grid = downsample(fine)
    case, mini = sn_files(assign, grid)
    if os.environ.get("OPENBNCT_REG_SN_FILES_ONLY"):  # write the S_N inputs, skip MC
        for name, obj in (("sn-case.json", case), ("sn-assignment.json", mini)):
            with open(os.path.join(HERE, name), "w") as f:
                json.dump(obj, f, separators=(",", ":") if "assign" in name else None,
                          indent=None if "assign" in name else 1)
                f.write("\n")
        return
    work = common.workdir(TAG)
    xs = common.build_cross_sections(work)
    model = build_model(grid, xs, work)
    sp_path = common.run_model(model, work)

    with openmc.StatePoint(sp_path) as sp:
        tf = sp.get_tally(name="flux-bands")
        tb = sp.get_tally(name="abs-b10")
        tn = sp.get_tally(name="abs-n14")
        vol = DX_CM**3
        # Bins: mesh (x fastest, then y, z), then energy band. Raw tallies are
        # per source neutron; flux [cm] / voxel volume = density [1/cm2].
        fm = tf.mean.reshape(N**3, 3) / vol
        fs = tf.std_dev.reshape(N**3, 3) / vol
        bm, bs = tb.mean.reshape(N**3), tb.std_dev.reshape(N**3)  # absorptions per voxel
        nm_, ns = tn.mean.reshape(N**3), tn.std_dev.reshape(N**3)

    # Flat voxel index = i + N*j + N*N*k (x fastest) for both the tallies and
    # the S_N cell numbering. Sanity: N14 absorbs only in tissue voxels, and
    # nearly every tissue voxel scores.
    flat = np.array([grid[i, j, k] for k in range(N) for j in range(N) for i in range(N)])
    assert nm_[flat == 0].sum() < 1e-9 * nm_.sum(), "absorption in void voxels: lattice orientation wrong"
    tissue = flat > 0
    assert (nm_[tissue] > 0).mean() > 0.9
    # Sanity: source enters +z with full disk weight: entrance-slab fast flux > 0
    # in the central column and the top-z voxels lie in the shadow (less flux).
    zsl = np.arange(N**3) // (N * N)
    assert fm[zsl == 0, 2].sum() > fm[zsl == N - 1, 2].sum()

    ref = {
        "schema_version": "openbnct.regression-reference/0.1.0",
        "case": TAG,
        "engine": f"openmc {openmc.__version__} continuous-energy endfb-viii.1 + c_H_in_H2O",
        "histories": PARTICLES * BATCHES,
        "seed": SEED,
        "normalization": "per source neutron. flux = track length / voxel volume [1/cm2]; "
        "B10/N14 absorption = reactions per voxel per source neutron (NOT per cm3).",
        "shape": [N, N, N],
        "voxel_index": "i + N*j + N*N*k",
        "voxel_volume_cm3": vol,
        "material_index": {"0": "void", "1": "skin", "2": "skull", "3": "brain"},
        "material_of_voxel": [int(v) for v in flat],
        "band_edges_ev": [common.SRC_BOUNDS_EV[0], common.THERMAL_MAX_EV,
                          common.EPITHERMAL_MAX_EV, common.SRC_BOUNDS_EV[3]],
        "flux_mean_voxel_x_band": common.fmt(fm),
        "flux_std_voxel_x_band": common.fmt(fs),
        "b10_absorption_mean": common.fmt(bm),
        "b10_absorption_std": common.fmt(bs),
        "n14_absorption_mean": common.fmt(nm_),
        "n14_absorption_std": common.fmt(ns),
    }
    with open(os.path.join(HERE, "reference.json"), "w") as f:
        json.dump(ref, f, separators=(",", ":"))
        f.write("\n")
    with open(os.path.join(HERE, "sn-case.json"), "w") as f:
        json.dump(case, f, indent=1)
        f.write("\n")
    with open(os.path.join(HERE, "sn-assignment.json"), "w") as f:
        json.dump(mini, f, separators=(",", ":"))
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
            "materials": [
                f"benchmarks/synthetic/layered-head-phantom/materials/{n}.json"
                for n in ("skin", "skull", "brain")
            ],
            "source_assignment": "benchmarks/synthetic/layered-head-phantom/assignment.json",
            "source_assignment_sha256": common.sha256(
                os.path.join(common.BENCH, "assignment.json")
            ),
        },
    )
    for m, nm in MAT_NAMES.items():
        s = flat == m
        print(nm, s.sum(), "voxels; N14", nm_[s].sum(), "B10", bm[s].sum(),
              "flux bands", fm[s].sum(axis=0))


if __name__ == "__main__":
    main()

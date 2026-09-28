#!/usr/bin/env python3
"""Same-data MG comparison: OpenMC multigroup mode on the FiR-1
cylindrical phantom with OpenBNCT's own collapsed 28-group constants.

Every input is taken verbatim from the committed S_N solve inputs so
the only remaining degrees of freedom are the transport algorithm and
residual conventions:

  data     validation/fir1-k63-cylindrical-phantom/multigroup-data-28g-tsl-v4.json
           (sigma_total, scatter P0+P1 moments — Legendre order=1 —
           absorption derived as sigma_t - row-sum(sigma_s))
  geometry validation/fir1-k63-cylindrical-phantom/case.json +
           assignment.json — voxel set is a cell-center-inside
           rasterization of ZCylinder(r=10, centre=(0.5,0.5)cm),
           z in [0.5, 24.5] cm; rest void.
  source   case.json: r=7 cm disk at z=0, cone half-angle 0.1491 rad
           about +z; group weights replicate the solver's
           collapse_consistent within-bin spread of the declared
           3-bin histogram.
  tally    RegularMesh on the exact S_N voxel grid (24x24x26 @ 1 cm,
           origin (-11.5,-11.5,0.5) cm), EnergyFilter on the same
           group edges, track-length flux score.

Conventions verified by convention_probe.py in this directory:
scatter_matrix[i][j] is i->j row-major (matches
scatter_matrix_per_cm[g*G+g']), EnergyGroups edges ascending, MG
source energies sampled in eV then binned.

Residual confound: the MC geometry is the analytic cylinder; the S_N
geometry is its cell-centre rasterization. Rim cells (fractional fill
in MC, all-or-nothing in S_N) are flagged in the comparison rather
than hidden.
"""
import argparse
import json
import shutil
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
PHANTOM = ROOT / "validation/fir1-k63-cylindrical-phantom"
DATA_JSON = PHANTOM / "multigroup-data-28g-tsl-v4.json"
CASE_JSON = PHANTOM / "case.json"
OUT = Path(__file__).resolve().parent

KT_EV = 0.0253
CUT_EV = 0.5


def spectrum_weight(lo: float, hi: float) -> float:
    """Replicates multigroup.rs spectrum_weight_integral CollapseConsistent."""
    lo = max(lo, 1.0e-30)
    hi = max(hi, lo)
    mid = min(max(CUT_EV, lo), hi)
    w = 0.0
    if mid > lo:
        a, b = lo, mid
        w += KT_EV * (KT_EV + a) * np.exp(-a / KT_EV) - KT_EV * (KT_EV + b) * np.exp(-b / KT_EV)
    if hi > mid:
        w += np.log(hi / max(mid, 1.0e-30))
    return w


def group_weights(case: dict, bounds_desc: list[float]) -> np.ndarray:
    """Per-group emission weights under collapse_consistent spread."""
    g = len(bounds_desc) - 1
    w = np.zeros(g)
    energy = case["source"]["energy"]
    edges = energy["energy_boundaries_ev"]
    for i, bw in enumerate(energy["bin_weights"]):
        lo, hi = edges[i], edges[i + 1]
        norm = spectrum_weight(lo, hi)
        if norm <= 0.0:
            continue
        for k in range(g):
            glo, ghi = bounds_desc[k + 1], bounds_desc[k]
            olo, ohi = max(lo, glo), min(hi, ghi)
            if ohi > olo:
                w[k] += bw * spectrum_weight(olo, ohi) / norm
    return w / w.sum()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--histories", type=int, default=200_000)
    ap.add_argument("--batches", type=int, default=10)
    ap.add_argument("--outdir", default=str(OUT / "run"))
    ap.add_argument("--p1", action="store_true", help="carry P1 scatter moments (order=1)")
    args = ap.parse_args()
    outdir = Path(args.outdir)
    outdir.mkdir(parents=True, exist_ok=True)

    import openmc

    data = json.loads(DATA_JSON.read_text())
    case = json.loads(CASE_JSON.read_text())
    bounds_desc = data["energy_boundaries_ev"]  # descending, g=0 hottest
    G = len(bounds_desc) - 1
    bounds_asc = sorted(bounds_desc)

    groups = openmc.mgxs.EnergyGroups(bounds_asc)

    # ---- materials ----------------------------------------------------
    lib = openmc.MGXSLibrary(groups)
    mat_objs = []
    for m in data["materials"]:
        st = np.array(m["sigma_total_per_cm"])
        s0 = np.array(m["scatter_matrix_per_cm"]).reshape(G, G)
        absxs = st - s0.sum(axis=1)
        xs = openmc.XSdata(m["material_id"], groups)
        if args.p1 and "scatter_p1_matrix_per_cm" in m:
            s1 = np.array(m["scatter_p1_matrix_per_cm"]).reshape(G, G)
            xs.scatter_format = "legendre"
            xs.order = 1
            sm = np.stack([s0, s1], axis=-1)  # [G][G'][l]
            xs.set_scatter_matrix(sm)
        else:
            xs.scatter_format = "legendre"
            xs.order = 0
            xs.set_scatter_matrix(s0[:, :, None])
        xs.set_total(st)
        xs.set_absorption(np.maximum(absxs, 0.0))
        lib.add_xsdata(xs)
        mat = openmc.Material(name=m["material_id"])
        mat.set_density("macro", 1.0)
        mat.add_macroscopic(m["material_id"])
        mat_objs.append(mat)
    lib.export_to_hdf5(outdir / "mgxs.h5")

    mats = openmc.Materials(mat_objs)
    mats.cross_sections = str(outdir / "mgxs.h5")
    mats.export_to_xml(outdir / "materials.xml")

    # ---- geometry -----------------------------------------------------
    # origin_mm is a cell-centre coordinate: the domain edge is
    # origin - spacing/2 (multigroup.rs ~line 815). The committed 24x24x26
    # @ 1 cm grid from centres (-115,-115,5) mm spans x,y in [-12,12] cm,
    # z in [0,26] cm — verified against the committed voxel_set. The water
    # cylinder is r=10 cm centred (0,0) over z in [0,24] cm; the rest is
    # void. The beam plane z=0 is the domain's z-low face AND the
    # cylinder's near face: birth at z=1e-6 cm (inside the boundary by
    # 10 nm — numerically identical, avoids surface ambiguity).
    water = mat_objs[0]
    void = mat_objs[1]
    xlo, ylo, zlo = -12.0, -12.0, 0.0
    xhi, yhi, zhi = 12.0, 12.0, 26.0
    walls = [
        openmc.XPlane(xlo, boundary_type="vacuum"),
        openmc.XPlane(xhi, boundary_type="vacuum"),
        openmc.YPlane(ylo, boundary_type="vacuum"),
        openmc.YPlane(yhi, boundary_type="vacuum"),
        openmc.ZPlane(zlo, boundary_type="vacuum"),
        openmc.ZPlane(zhi, boundary_type="vacuum"),
    ]
    box = +walls[0] & -walls[1] & +walls[2] & -walls[3] & +walls[4] & -walls[5]
    cyl = openmc.ZCylinder(x0=0.0, y0=0.0, r=10.0)
    zc0 = openmc.ZPlane(0.0)
    zc1 = openmc.ZPlane(24.0)
    water_cell = openmc.Cell(fill=water, region=box & -cyl & +zc0 & -zc1)
    void_cell = openmc.Cell(fill=void, region=box & ~water_cell.region)
    openmc.Geometry([water_cell, void_cell]).export_to_xml(outdir / "geometry.xml")

    # ---- source -------------------------------------------------------
    w = group_weights(case, bounds_desc)
    ba = np.asarray(bounds_asc)
    repr_ev = np.sqrt(ba[:-1] * ba[1:])  # gm midpoint per ASC bin
    # our group g (desc order, 0=hottest) -> ascending bin index G-1-g
    x_ev = [float(repr_ev[G - 1 - k]) for k in range(G)]
    src = openmc.IndependentSource()
    src.space = openmc.stats.CylindricalIndependent(
        r=openmc.stats.PowerLaw(0.0, 7.0, 1.0),
        phi=openmc.stats.Uniform(0.0, 2.0 * np.pi),
        z=openmc.stats.Discrete([1.0e-6], [1.0]),  # 10 nm inside z=0 face
    )
    src.angle = openmc.stats.PolarAzimuthal(
        mu=openmc.stats.Uniform(np.cos(0.1491), 1.0),
        phi=openmc.stats.Uniform(0.0, 2.0 * np.pi),
        reference_uvw=(0.0, 0.0, 1.0),
    )
    src.energy = openmc.stats.Discrete(x_ev, [float(v) for v in w])
    src.strength = 1.0

    settings = openmc.Settings()
    settings.energy_mode = "multi-group"
    settings.tabular_legendre = {"enable": False}
    settings.run_mode = "fixed source"
    settings.batches = args.batches
    settings.particles = args.histories
    settings.source = src
    settings.export_to_xml(outdir / "settings.xml")

    # ---- tallies ------------------------------------------------------
    mesh = openmc.RegularMesh()
    mesh.lower_left = [-12.0, -12.0, 0.0]
    mesh.upper_right = [12.0, 12.0, 26.0]
    mesh.dimension = [24, 24, 26]
    tally = openmc.Tally(name="cell_flux")
    tally.filters = [openmc.MeshFilter(mesh), openmc.EnergyFilter(bounds_asc)]
    tally.scores = ["flux"]
    tally.estimator = "tracklength"
    openmc.Tallies([tally]).export_to_xml(outdir / "tallies.xml")

    manifest = {
        "data_json": str(DATA_JSON.relative_to(ROOT)),
        "case_json": str(CASE_JSON.relative_to(ROOT)),
        "histories": args.histories,
        "batches": args.batches,
        "p1": args.p1,
        "group_weights_our_g_order": w.tolist(),
        "source_repr_ev": x_ev,
    }
    (outdir / "manifest.json").write_text(json.dumps(manifest, indent=1))

    rc = openmc.run(cwd=outdir, openmc_exec=shutil.which("openmc"))
    print("openmc rc:", rc)
    return int(rc or 0)


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Convention probe for OpenMC 0.15.3 multi-group mode.

Two conventions must be pinned before the same-data comparison can be
trusted:

1. Source group indexing — `Discrete` energy distribution group
   numbers: are they 0-based or 1-based, and which end is high energy?
2. Scatter-matrix orientation — is `scatter_matrix[i][j]` read as
   i→j (row=in-group) or j→i (row=out-group)?

Probe: a 2-group, 1 cm³ box of material with pure downscatter
(g1→g2 only), mono-group source at the top of the structure. If
conventions are as documented, all flux lands in group 2.
"""
import json
import sys
import tempfile
from pathlib import Path

import numpy as np
import openmc

GROUPS = 2
BOUNDS_EV = [1.0e-5, 1.0, 1.0e7]  # ascending; openmc g1 = top bin


def build(orientation: str, source_ev: float, outdir: Path) -> Path:
    groups = openmc.mgxs.EnergyGroups(BOUNDS_EV)
    xs = openmc.XSdata("mat", groups)
    xs.scatter_format = "legendre"
    xs.order = 0
    xs.set_total([1.0, 1.0])
    xs.set_absorption([0.0, 0.0])  # pure scatterer — must conserve
    sm = np.zeros((GROUPS, GROUPS, 1))
    if orientation == "in_out":
        sm[0, 1, 0] = 1.0  # row=in g1 -> col=out g2
        sm[1, 1, 0] = 1.0  # g2 terminal: self-scatter keeps pdf valid
    else:
        sm[1, 0, 0] = 1.0  # row=out g2 <- col=in g1
        sm[0, 0, 0] = 1.0  # g1 terminal: self-scatter keeps pdf valid
    xs.set_scatter_matrix(sm)

    lib = openmc.MGXSLibrary(groups)
    lib.add_xsdata(xs)
    lib.export_to_hdf5(outdir / "mgxs.h5")

    mat = openmc.Material(name="mat")
    mat.set_density("macro", 1.0)
    mat.add_macroscopic("mat")
    mats = openmc.Materials([mat])
    mats.cross_sections = str(outdir / "mgxs.h5")
    mats.export_to_xml(outdir / "materials.xml")

    s = openmc.ZPlane(0.0, boundary_type="vacuum")
    e = openmc.ZPlane(1.0, boundary_type="vacuum")
    x0 = openmc.XPlane(0.0, boundary_type="vacuum")
    x1 = openmc.XPlane(1.0, boundary_type="vacuum")
    y0 = openmc.YPlane(0.0, boundary_type="vacuum")
    y1 = openmc.YPlane(1.0, boundary_type="vacuum")
    cell = openmc.Cell(fill=mat, region=+s & -e & +x0 & -x1 & +y0 & -y1)
    geo = openmc.Geometry([cell])
    geo.export_to_xml(outdir / "geometry.xml")

    src = openmc.IndependentSource()
    src.space = openmc.stats.Box([0.4, 0.4, 0.4], [0.6, 0.6, 0.6])
    src.angle = openmc.stats.Isotropic()
    src.energy = openmc.stats.Discrete([source_ev], [1.0])

    settings = openmc.Settings()
    settings.energy_mode = "multi-group"
    settings.tabular_legendre = {"enable": False}
    settings.batches = 5
    settings.particles = 2000
    settings.source = src
    settings.run_mode = "fixed source"
    settings.export_to_xml(outdir / "settings.xml")

    mesh = openmc.RegularMesh()
    mesh.lower_left = [0.0, 0.0, 0.0]
    mesh.upper_right = [1.0, 1.0, 1.0]
    mesh.dimension = [1, 1, 1]
    mf = openmc.MeshFilter(mesh)
    ef = openmc.EnergyFilter(sorted(BOUNDS_EV))
    tally = openmc.Tally(name="probe")
    tally.filters = [mf, ef]
    tally.scores = ["flux"]
    tallies = openmc.Tallies([tally])
    tallies.export_to_xml(outdir / "tallies.xml")


def read_sp(path: Path):
    with openmc.StatePoint(str(sorted(path.glob("statepoint.*.h5"))[-1])) as sp:
        t = sp.get_tally(name="probe")
        # mean array: [energy-bin, mesh-cell, score]
        return t.mean.flatten()


def main():
    base = Path(sys.argv[1] if len(sys.argv) > 1 else tempfile.mkdtemp())
    base.mkdir(parents=True, exist_ok=True)
    results = {}
    # Source at 5e6 eV (group 1, high bin). With pure downscatter all
    # particles must end in the low bin under the correct orientation.
    for label, orientation, sg in [
        ("inout", "in_out", 5.0e6),
        ("outin", "out_in", 5.0e6),
    ]:
        d = base / label
        d.mkdir(exist_ok=True)
        build(orientation, sg, d)
        openmc.run(cwd=d, openmc_exec=shutil_which(), output=False)
        mean = read_sp(d)
        # EnergyFilter bins ascending: [1e-5,1) then [1,1e7)
        results[label] = {"lo": float(mean[0]), "hi": float(mean[1])}
        print(f"{label}: lo={mean[0]:.3e} hi={mean[1]:.3e}")
    (base / "probe_results.json").write_text(json.dumps(results, indent=1))


def shutil_which():
    import shutil

    return shutil.which("openmc")


if __name__ == "__main__":
    main()

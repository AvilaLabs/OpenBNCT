#!/usr/bin/env python3
"""Compare OpenMC MG mesh tally against the committed S_N flux field.

Energy ordering: OpenMC EnergyFilter bins are ascending eV; our group
index is descending (g=0 hottest). Tally bin e -> our g = G-1-e.

Mesh ordering: OpenMC RegularMesh flattens x fastest
(i + nx*j + nx*ny*k), identical to our cell indexing.

Normalization: OpenMC 'flux' score is track-length flux per source
particle (volume-integrated: cm). Our artifact stores cm^-2 s^-1 per
unit source rate = track-length/cm^3 per source. Divide the tally by
the 1 cm^3 voxel volume to compare.
"""
import json
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
PHANTOM = ROOT / "validation/fir1-k63-cylindrical-phantom"


def load_sn(path: Path):
    f = json.loads(path.read_text())
    return np.array(f["flux"])  # [cell][group], cell = i + 24j + 624k


def load_mc(sp_path: Path):
    import openmc

    with openmc.StatePoint(str(sp_path)) as sp:
        t = sp.get_tally(name="cell_flux")
        mean = t.mean.reshape(-1)  # [mesh?][e?] flattened
        std = t.std_dev.reshape(-1)
    n_cells = 24 * 24 * 26
    n_groups = len(mean) // n_cells
    # filters=[MeshFilter, EnergyFilter]: energy varies fastest
    mean = mean.reshape(n_cells, n_groups)
    std = std.reshape(n_cells, n_groups)
    # convert volume-integrated track length -> flux (V=1 cm^3)
    return mean[:, ::-1] / 1.0, std[:, ::-1] / 1.0  # flip to desc groups


def cell_xyz(c):
    return c % 24, (c // 24) % 24, c // (24 * 24)


def main():
    sp = Path(sys.argv[1])
    sn_path = Path(sys.argv[2]) if len(sys.argv) > 2 else (
        PHANTOM / "multigroup-flux-28g-tsl-p1-1e.json")
    sn = load_sn(sn_path)
    mc, mc_std = load_mc(sp)
    G = sn.shape[1]

    # water-mask: rasterized cylinder per assignment — cells in the set
    assign = json.loads((PHANTOM / "assignment.json").read_text())
    mask = np.zeros(sn.shape[0], dtype=bool)
    for ijk in assign["regions"][0]["shape"]["indices"]:
        mask[ijk[0] + 24 * ijk[1] + 24 * 24 * ijk[2]] = True

    band_names = {
        "fast (g<11)": (0, 11),
        "epi (11<=g<22)": (11, 22),
        "thermal (g>=22)": (22, 28),
    }
    print(f"cells: {sn.shape[0]}, groups: {G}, water cells: {mask.sum()}")
    print(f"{'band':<18}{'z-slab':<8}{'SN/Mc median':<14}{'p16':<8}{'p84':<8}{'rel-std mc'}")
    report = {"groups": G, "cells": int(sn.shape[0]), "bands": {}}
    for name, (g0, g1) in band_names.items():
        rows = []
        for k in range(24):
            cells = [c for c in range(sn.shape[0]) if mask[c] and cell_xyz(c)[2] == k]
            if not cells:
                continue
            s = sn[np.ix_(cells, range(g0, g1))].mean(axis=1)
            m = mc[np.ix_(cells, range(g0, g1))].mean(axis=1)
            ok = (s > 0) & (m > 0)
            if ok.sum() == 0:
                continue
            r = s[ok] / m[ok]
            rel = (mc_std[np.ix_(cells, range(g0, g1))].mean(axis=1) / m)[ok]
            rows.append((k, np.median(r), np.percentile(r, 16), np.percentile(r, 84), np.median(rel)))
            report["bands"].setdefault(name, []).append(
                {"z": k, "sn_mc_median": float(np.median(r)),
                 "p16": float(np.percentile(r, 16)), "p84": float(np.percentile(r, 84)),
                 "mc_relstd": float(np.median(rel))})
        for row in rows:
            print(f"{name:<18}{row[0]:<8}{row[1]:<14.3f}{row[2]:<8.3f}{row[3]:<8.3f}{row[4]:.3f}")

    # balance: total absorption vs MC leakage/stealing absent — compare
    # phantom-integrated flux totals
    tot_sn = sn[mask].sum()
    tot_mc = mc[mask].sum()
    print(f"\nphantom-integrated flux-volume: SN={tot_sn:.4e}  MC={tot_mc:.4e}  ratio={tot_sn/tot_mc:.3f}")
    report["phantom_flux_volume_ratio_sn_mc"] = float(tot_sn / tot_mc)
    out = Path(sys.argv[3]) if len(sys.argv) > 3 else sp.parent / "comparison.json"
    out.write_text(json.dumps(report, indent=1))
    print("wrote", out)


if __name__ == "__main__":
    main()

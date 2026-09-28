#!/usr/bin/env python3
"""h-refinement arm: compare a 5 mm S_N solve against the same-data MC
tally (which lives on the 1 cm grid — the MC transport is mesh-free, so
coarsening the S_N field onto the 1 cm mask is the honest comparison).

Coarsening rule: each 5 mm cell maps to the 1 cm cell containing its
centre. The 5 mm grid is NOT commensurate with the 1 cm grid (5 mm
centres fall on 1 cm *faces*), so each 1 cm cell's value is the
volume-weighted mean of the 4^? — actually 8 5-mm cells straddling its
centre neighbourhood; we just average all 5 mm cells whose centres lie
inside each 1 cm cell's extent. That gives the cell-average flux on the
coarse volume directly — the correct comparison quantity.
"""
import json
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
PHANTOM = ROOT / "validation/fir1-k63-cylindrical-phantom"
FINE = ROOT / "target/mg-fine"


def main():
    sp = Path(sys.argv[1])
    fine_path = Path(sys.argv[2]) if len(sys.argv) > 2 else FINE / "flux-5mm-p1.json"

    sn5 = np.array(json.loads(fine_path.read_text())["flux"])  # [48*48*52][28]
    case5 = json.loads((FINE / "case-5mm.json").read_text())
    nx5 = case5["geometry"]["shape"]  # [48,48,52]
    sp5 = np.array(case5["geometry"]["spacing_mm"])
    org5 = np.array(case5["geometry"]["origin_mm"])  # cell-centre of cell 0
    # cell centres in cm
    c5 = np.empty((sn5.shape[0], 3))
    for c in range(sn5.shape[0]):
        i = c % nx5[0]; j = (c // nx5[0]) % nx5[1]; k = c // (nx5[0] * nx5[1])
        c5[c] = (org5 + sp5 * np.array([i, j, k])) / 10.0

    # 1 cm grid cells: centres (-11.5..11.5, ..., 0.5..25.5)
    idx = np.round((c5 - np.array([-11.5, -11.5, 0.5]))).astype(int)
    ok = np.all((idx >= 0) & (idx < np.array([24, 24, 26])), axis=1)
    idx = idx[ok]
    lin = idx[:, 0] + 24 * idx[:, 1] + 576 * idx[:, 2]
    vals = sn5[ok]

    # coarsen: mean of fine cells per coarse cell
    n_coarse = 24 * 24 * 26
    acc = np.zeros((n_coarse, sn5.shape[1])); cnt = np.zeros(n_coarse)
    np.add.at(acc, lin, vals); np.add.at(cnt, lin, 1)
    sn = acc / np.maximum(cnt, 1)[:, None]

    import openmc
    with openmc.StatePoint(str(sp)) as s:
        t = s.get_tally(name="cell_flux")
        mc = t.mean.reshape(-1).reshape(n_coarse, 28)[:, ::-1]
        mcstd = t.std_dev.reshape(-1).reshape(n_coarse, 28)[:, ::-1]

    assign = json.loads((PHANTOM / "assignment.json").read_text())
    mask = np.zeros(n_coarse, bool)
    for ijk in assign["regions"][0]["shape"]["indices"]:
        mask[ijk[0] + 24 * ijk[1] + 576 * ijk[2]] = True
    cells = np.where(mask)[0]
    kk = np.array([c // 576 for c in cells])

    for name, g0, g1 in [("fast", 0, 11), ("epi", 11, 22), ("thermal", 22, 28)]:
        for z in [0, 1, 3, 6, 9, 12, 15, 18, 21, 23]:
            sel = kk == z
            s = sn[cells[sel], g0:g1].mean(1); m = mc[cells[sel], g0:g1].mean(1)
            good = m > 0
            r = s[good] / m[good]
            rel = np.median(mcstd[cells[sel], g0:g1].mean(1)[good] / m[good])
            print(f"{name:<8} z={z:2d}  SN5/MC med={np.median(r):.3f}  p16={np.percentile(r,16):.3f}  p84={np.percentile(r,84):.3f}  mc_relstd={rel:.3f}")
    print(f"phantom flux-volume: SN5={sn[mask].sum():.4e} MC={mc[mask].sum():.4e} ratio={sn[mask].sum()/mc[mask].sum():.3f}")


if __name__ == "__main__":
    main()

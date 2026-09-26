#!/usr/bin/env python3
"""Score Reed's-problem flux against the Warsa (2002) reference.

Reads the two solve artifacts (src1: strong absorber/source slab;
src2: one copy of the weak scattering source — the mirror copy is
supplied by x -> -x reflection), superposes them, and compares the
transverse-averaged scalar flux at cell centers on x in [0,8] cm to
the eigenfunction reference.

Two metrics, both reported:

1. Region-averaged scalar flux per material region — the quantity
   Reed's paper tabulates; the reference is trapezoid-integrated on
   its own 0.1 cm grid.
2. Per-cell relative error vs the linearly interpolated pointwise
   reference, restricted to interior cells (>=1 cell from any material
   boundary) with phi_ref > PHI_FLOOR, and excluding the absorber
   region (2,3): inside its steep exponential dip the 0.1 cm reference
   grid cannot represent the within-cell shape, so pointwise
   interpolation is not a valid comparator there.

Acceptance thresholds are declared below and reproduced in README.md.
"""

import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
# Flux artifacts live here by default; REED_OUT points to a refinement
# run's directory (generate.py honors the same variable).
DATA_DIR = os.environ.get("REED_OUT", HERE)
NX = int(os.environ.get("REED_NX", "160"))
NY, NZ = 4, 4
DX_CM = 16.0 / NX
X0_CM = -8.0 + DX_CM / 2.0  # cell-0 center

PHI_FLOOR = 0.05
# Region-mean acceptance thresholds (declared, not fit): the flat
# regions must be within a percent-ish of the eigenfunction reference;
# the absorber dip is reference-resolution-limited (its trapezoid
# estimate overshoots the true region mean under a convex drop), and
# the curved src2 peak carries the angular-quadrature truncation that
# an S_N method exhibits at S8 (~2%, documented to halve on order
# refinement — see README).
REGION_TOL = {
    "src1": 0.02,
    "abs1": 0.10,
    "void": 0.015,
    "src2": 0.04,
    "refl": 0.01,
}
# Interior per-cell acceptance (excludes abs1 and boundary-adjacent
# cells).
MAX_REL_ERR = 0.05
RMS_REL_ERR = 0.02

REGIONS = [
    ("src1", 0.0, 2.0),
    ("abs1", 2.0, 3.0),
    ("void", 3.0, 5.0),
    ("src2", 5.0, 6.0),
    ("refl", 6.0, 8.0),
]


def load_reference():
    xs, phis = [], []
    with open(os.path.join(HERE, "reference", "warsa-2002-eigenfunction.csv")) as fh:
        for line in fh:
            line = line.strip()
            if line:
                x, p = line.split(",")
                xs.append(float(x))
                phis.append(float(p))
    return xs, phis


def interp(xs, phis, x):
    if x <= xs[0]:
        return phis[0]
    if x >= xs[-1]:
        return phis[-1]
    lo, hi = 0, len(xs) - 1
    while hi - lo > 1:
        mid = (lo + hi) // 2
        if xs[mid] <= x:
            lo = mid
        else:
            hi = mid
    t = (x - xs[lo]) / (xs[hi] - xs[lo])
    return phis[lo] + t * (phis[hi] - phis[lo])


def ref_region_avg(xs, phis, xlo, xhi):
    pts = [(x, p) for x, p in zip(xs, phis) if xlo - 1e-9 <= x <= xhi + 1e-9]
    area = sum(
        (pts[i + 1][1] + pts[i][1]) / 2.0 * (pts[i + 1][0] - pts[i][0])
        for i in range(len(pts) - 1)
    )
    return area / (xhi - xlo)


def column(flux_path):
    """Transverse-averaged group-0 flux, one value per x-cell."""
    doc = json.load(open(flux_path))
    flux = doc["flux"]  # [cell][group], cell = i + NX*j + NX*NY*k
    col = []
    for i in range(NX):
        acc = 0.0
        for k in range(NZ):
            for j in range(NY):
                acc += flux[i + NX * j + NX * NY * k][0]
        col.append(acc / (NY * NZ))
    return col


def region_of(x):
    for name, lo, hi in REGIONS:
        if lo <= x < hi:
            return name
    return "refl"


def main():
    c1 = column(os.path.join(DATA_DIR, "flux-src1.json"))
    c2 = column(os.path.join(DATA_DIR, "flux-src2.json"))
    xs, ref = load_reference()

    # Cell index of a boundary edge |x|=b on the right half:
    # edge at x = b lies between cells i=b/DX-1 and i=b/DX + NX/2 offset.
    def cells_of(lo, hi):
        return [
            i
            for i in range(NX // 2, NX)
            if lo - 1e-9 <= X0_CM + DX_CM * i < hi + 1e-9
        ]

    tot = [c1[i] + c2[i] + c2[NX - 1 - i] for i in range(NX)]

    region_rows = []
    fails = []
    for name, lo, hi in REGIONS:
        cells = cells_of(lo, hi)
        ours = sum(tot[i] for i in cells) / len(cells)
        r = ref_region_avg(xs, ref, lo, hi)
        rel = abs(ours - r) / r
        ok = rel <= REGION_TOL[name]
        region_rows.append(
            {
                "region": name,
                "x_range_cm": [lo, hi],
                "n_cells": len(cells),
                "phi_mean": ours,
                "reference_mean": r,
                "rel_err": rel,
                "tol": REGION_TOL[name],
                "pass": ok,
            }
        )
        if not ok:
            fails.append(name)

    # Interior pointwise comparison: skip cells adjacent to material
    # boundaries (within 1 cell of a boundary edge) and the absorber
    # region, where the reference grid is too coarse to interpolate.
    edge_idx = set()
    for _, lo, hi in REGIONS:
        for b in (lo, hi):
            if b <= 0.0 or b >= 8.0:
                continue
            i_edge = round((b - X0_CM) / DX_CM)  # cell whose center ~ b
            edge_idx.update([i_edge - 1, i_edge])
    rows = []
    for i in range(NX // 2, NX):
        x = X0_CM + DX_CM * i
        reg = region_of(x)
        if reg == "abs1" or i in edge_idx:
            continue
        r = interp(xs, ref, x)
        rel = abs(tot[i] - r) / r if r > 0 else float("inf")
        rows.append(
            {
                "x_cm": round(x, 4),
                "region": reg,
                "phi": tot[i],
                "reference": r,
                "rel_err": rel,
            }
        )

    scored = [r for r in rows if r["reference"] > PHI_FLOOR]
    max_rel = max(r["rel_err"] for r in scored)
    rms_rel = (sum(r["rel_err"] ** 2 for r in scored) / len(scored)) ** 0.5
    worst = max(scored, key=lambda r: r["rel_err"])

    verdict = (
        "pass"
        if (not fails and max_rel <= MAX_REL_ERR and rms_rel <= RMS_REL_ERR)
        else "fail"
    )
    report = {
        "schema_version": "openbnct.validation-comparison/0.1.0",
        "case": "canonical-reed-problem",
        "reference": "Warsa 2002 eigenfunction expansion (ReedSol.csv, "
        "DrRyanMc/Benchmarks), pointwise phi on x in [0,8] cm",
        "scoring": {
            "region_tolerances": REGION_TOL,
            "phi_floor": PHI_FLOOR,
            "max_rel_err_threshold": MAX_REL_ERR,
            "rms_rel_err_threshold": RMS_REL_ERR,
            "interior_only": "per-cell metric excludes cells adjacent to "
            "material boundaries and the steep absorber dip",
        },
        "region_means": region_rows,
        "n_cells_scored": len(scored),
        "max_rel_err": max_rel,
        "rms_rel_err": rms_rel,
        "worst_cell": worst,
        "verdict": verdict,
        "cells": rows,
    }
    out = os.path.join(DATA_DIR, "comparison.json")
    with open(out, "w") as fh:
        json.dump(report, fh, indent=2)
        fh.write("\n")
    for r in region_rows:
        print(
            f"  {r['region']:>5} [{r['x_range_cm'][0]}-{r['x_range_cm'][1]}] "
            f"ours={r['phi_mean']:.4f} ref={r['reference_mean']:.4f} "
            f"rel={r['rel_err'] * 100:.2f}% tol={r['tol'] * 100:.0f}% "
            f"{'ok' if r['pass'] else 'FAIL'}"
        )
    print(
        f"interior: max_rel={max_rel:.4f} rms_rel={rms_rel:.4f} "
        f"worst x={worst['x_cm']} ({worst['region']})  verdict={verdict}"
    )
    sys.exit(0 if verdict == "pass" else 1)


if __name__ == "__main__":
    main()

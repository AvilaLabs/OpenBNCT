#!/usr/bin/env python3
"""Three-way intercomparison: S_N S8 (canonical-kobayashi-p1) vs
OpenMC MG vs the published GMVP reference, problem 1 case ii.

Metrics per probe:
  * S_N vs GMVP relative error (deterministic bias)
  * MC vs GMVP z-score under MC's own std_dev
  * S_N vs MC relative error — the practical cross-code delta
"""

import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
KOBA = os.path.join(HERE, "..", "canonical-kobayashi-p1")
CELL_CM = float(os.environ.get("KOB1_CELL_MM", "20.0")) / 10.0
N = int(round(200.0 / CELL_CM))
X0 = -100.0 + CELL_CM / 2.0

GMVP = {
    (5, 5, 5): 8.29260e0,
    (5, 15, 5): 1.87028e0,
    (5, 25, 5): 7.13986e-1,
    (5, 35, 5): 3.84685e-1,
    (5, 45, 5): 2.53984e-1,
    (5, 55, 5): 1.37220e-1,
    (5, 65, 5): 4.65913e-2,
    (5, 75, 5): 1.58766e-2,
    (5, 85, 5): 5.47036e-3,
    (5, 95, 5): 1.85082e-3,
    (15, 15, 15): 6.63233e-1,
    (25, 25, 25): 2.68828e-1,
    (35, 35, 35): 1.56683e-1,
    (45, 45, 45): 1.04405e-1,
    (55, 55, 55): 3.02145e-2,
}


def idx(pt):
    x, y, z = (float(c) for c in pt)
    return (
        int(round((x - X0) / CELL_CM))
        + N * int(round((y - X0) / CELL_CM))
        + N * N * int(round((z - X0) / CELL_CM))
    )


def main():
    sn = json.load(open(os.path.join(KOBA, "flux-ii.json")))["flux"]
    mc = json.load(open(os.path.join(HERE, "openmc-flux.json")))["probes"]

    rows = []
    for rec in mc:
        pt = tuple(rec["point_cm"])
        g = GMVP[pt]
        s = sn[idx(pt)][0]
        m, ms = rec["phi_mc"], rec["phi_mc_std"]
        rows.append(
            {
                "point_cm": list(pt),
                "phi_sn": s,
                "phi_mc": m,
                "phi_gmvp": g,
                "sn_vs_gmvp_pct": 100.0 * (s - g) / g,
                "mc_vs_gmvp_z": (m - g) / ms,
                "sn_vs_mc_pct": 100.0 * (s - m) / m,
            }
        )
        print(
            f"  {pt}: sn={s:.4e} mc={m:.4e}±{100*ms/m:.0f}% gmvp={g:.4e} "
            f"| sn-ref={rows[-1]['sn_vs_gmvp_pct']:+.1f}% "
            f"mc z={rows[-1]['mc_vs_gmvp_z']:+.1f} "
            f"sn-mc={rows[-1]['sn_vs_mc_pct']:+.1f}%"
        )

    report = {
        "schema_version": "openbnct.validation-comparison/0.1.0",
        "case": "intercomparison-kobayashi-p1-ii",
        "description": "S_N S8 vs OpenMC-MG (same 2cm cell lattice) vs "
        "published GMVP. MC error bars are batch std_dev; S_N is "
        "deterministic. Deltas beyond ~2-sigma of MC stats and beyond "
        "the GMVP reference expose the S_N ray-effect field structure.",
        "probes": rows,
    }
    out = os.path.join(HERE, "comparison-mc.json")
    with open(out, "w") as fh:
        json.dump(report, fh, indent=2)
        fh.write("\n")
    print("wrote", out)


if __name__ == "__main__":
    main()

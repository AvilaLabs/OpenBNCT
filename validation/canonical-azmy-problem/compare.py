#!/usr/bin/env python3
"""Score the Azmy (1988) quadrant means against the published values.

The published reference is three quadrant-mean scalar fluxes on the
quarter domain (Azmy 1988 NSE 98:29-40; the same numbers FeenoX's
azmy-structured.fee prints). On our full-domain realization each
published quadrant corresponds to a family of mirrored sub-regions:

    source quadrant  |x|<5, |y|<5                    -> 1.676
    edge quadrants   |x|>5 & |y|<5, or |x|<5 & |y|>5 -> 4.159e-2
    corner quadrants |x|>5 & |y|>5                   -> 1.992e-3

All members of a family are identical by x/y mirror and x<->y swap
symmetry, so the family mean is the published quadrant mean.

Emits comparison.json (per-region means, rel errors, verdict).
"""

import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DATA_DIR = os.environ.get("AZMY_OUT", HERE)
NXY = int(os.environ.get("AZMY_NXY", "80"))
NZ = 2
DXY = 20.0 / NXY
X0 = -10.0 + DXY / 2.0

# Azmy's published three-significant-figure quadrant means (source /
# edge / corner of the quarter domain).
REFERENCE = {
    "source": 1.676,
    "edge": 4.159e-2,
    "corner": 1.992e-3,
}
# FeenoX's structured-grid S4 run lands within ~0.3-5% of the
# reference (LLQ 1.676, LRQ 4.164e-2, URQ 1.991e-3). Our θ-WDD on a
# cell grid should land at least as well; the corner's tiny flux makes
# it the least-accurate absolute number.
TOL = {"source": 0.02, "edge": 0.05, "corner": 0.10}


def main():
    doc = json.load(open(os.path.join(DATA_DIR, "flux.json")))
    flux = doc["flux"]  # [cell][group], cell = i + NXY*j + NXY*NXY*k

    def cell(i, j):
        acc = 0.0
        for k in range(NZ):
            acc += flux[i + NXY * j + NXY * NXY * k][0]
        return acc / NZ

    buckets = {"source": [], "edge": [], "corner": []}
    for i in range(NXY):
        x = X0 + DXY * i
        for j in range(NXY):
            y = X0 + DXY * j
            in_src_x, in_src_y = abs(x) < 5.0, abs(y) < 5.0
            if in_src_x and in_src_y:
                buckets["source"].append(cell(i, j))
            elif in_src_x != in_src_y:
                buckets["edge"].append(cell(i, j))
            else:
                buckets["corner"].append(cell(i, j))

    rows = []
    fails = []
    for name in ("source", "edge", "corner"):
        ours = sum(buckets[name]) / len(buckets[name])
        ref = REFERENCE[name]
        rel = abs(ours - ref) / ref
        ok = rel <= TOL[name]
        rows.append(
            {
                "quadrant": name,
                "n_cells": len(buckets[name]),
                "phi_mean": ours,
                "reference": ref,
                "rel_err": rel,
                "tol": TOL[name],
                "pass": ok,
            }
        )
        if not ok:
            fails.append(name)

    # Symmetry diagnostics: the four corner-quadrant means must agree
    # (x/y mirror + swap) and the edge strips likewise — these are
    # solver self-consistency checks independent of the reference.
    def quadrant_mean(xlo, xhi, ylo, yhi):
        vals = [
            cell(i, j)
            for i in range(NXY)
            for j in range(NXY)
            if xlo - 1e-9 <= X0 + DXY * i < xhi + 1e-9
            and ylo - 1e-9 <= X0 + DXY * j < yhi + 1e-9
        ]
        return sum(vals) / len(vals)

    corners = [
        quadrant_mean(sx, sx + 5.0, sy, sy + 5.0)
        for sx in (-10.0, 5.0)
        for sy in (-10.0, 5.0)
    ]
    sources = [
        quadrant_mean(sx, sx + 5.0, sy, sy + 5.0)
        for sx in (-5.0, 0.0)
        for sy in (-5.0, 0.0)
    ]
    edges = [
        quadrant_mean(-10.0, -5.0, -5.0, 0.0),
        quadrant_mean(5.0, 10.0, -5.0, 0.0),
        quadrant_mean(-10.0, -5.0, 0.0, 5.0),
        quadrant_mean(5.0, 10.0, 0.0, 5.0),
        quadrant_mean(-5.0, 0.0, -10.0, -5.0),
        quadrant_mean(0.0, 5.0, -10.0, -5.0),
        quadrant_mean(-5.0, 0.0, 5.0, 10.0),
        quadrant_mean(0.0, 5.0, 5.0, 10.0),
    ]

    def spread(vals):
        m = sum(vals) / len(vals)
        return (max(vals) - min(vals)) / m if m else 0.0

    symmetry = {
        "source_family_rel_spread": spread(sources),
        "edge_family_rel_spread": spread(edges),
        "corner_family_rel_spread": spread(corners),
    }

    verdict = "pass" if not fails else "fail"
    report = {
        "schema_version": "openbnct.validation-comparison/0.1.0",
        "case": "canonical-azmy-problem",
        "reference": "Azmy 1988 NSE 98:29-40 quadrant means "
        "(source 1.676, edge 4.159e-2, corner 1.992e-3); same anchors "
        "as FeenoX azmy-structured.fee",
        "tolerances": TOL,
        "quadrant_means": rows,
        "symmetry": symmetry,
        "verdict": verdict,
    }
    out = os.path.join(DATA_DIR, "comparison.json")
    with open(out, "w") as fh:
        json.dump(report, fh, indent=2)
        fh.write("\n")
    for r in rows:
        print(
            f"  {r['quadrant']:>7} ours={r['phi_mean']:.4e} "
            f"ref={r['reference']:.4e} rel={r['rel_err'] * 100:.2f}% "
            f"tol={r['tol'] * 100:.0f}% {'ok' if r['pass'] else 'FAIL'}"
        )
    print(
        "symmetry spreads: "
        + " ".join(f"{k}={v:.2e}" for k, v in symmetry.items())
        + f"  verdict={verdict}"
    )
    sys.exit(0 if verdict == "pass" else 1)


if __name__ == "__main__":
    main()

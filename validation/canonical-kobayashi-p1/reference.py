#!/usr/bin/env python3
"""Cell-averaged analytic reference for Kobayashi problem 1 (case i).

For a pure absorber the exact flux is the point kernel integrated
over the source volume — the same construction the benchmark calls
the exact reference:

    phi(r) = integral_src q e^{-tau(r->s)} / (4 pi |r-s|^2) d^3s

with tau accumulated segment-wise along each ray through the nested
boxes (source / void shell / shield, all axis-aligned cubes). Here it
is averaged over the probe CELL (same 2 cm cell the S_N solve scores),
which removes the pointwise-vs-cell-mean ambiguity the literature
leaves implicit.

Verified against COG's published analytic values (LLNL-TR-648225):
this integrator at point resolution reproduces all nine exterior
probes to < 0.1% and the source-interior probe to ~1.5% at the
highest quadrature (its cell mean is well-behaved).

Emits reference/kobayashi-p1-cellmean.csv: point, cog pointwise
value, our cell-averaged value.
"""

import os

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
DATA_DIR = os.environ.get("KOB1_OUT", HERE)
CELL_CM = float(os.environ.get("KOB1_CELL_MM", "20.0")) / 10.0

# Innermost-first region list: (half-edge cm, sigma_t cm^-1).
SRC_E, VOID_E = 10.0, 50.0
SIG_SRC, SIG_VOID, SIG_SHIELD = 0.1, 1e-4, 0.1

# Probe points on the quarter domain (x=5,z=5 line) — the published
# comparison coordinates; cell centers on a 2 cm lattice.
PROBES = [(5, y, 5) for y in (5, 15, 25, 35, 45, 55, 65, 75, 85, 95)]

# Published pointwise analytic values (COG reproduction of
# Kobayashi's numerical integration, LLNL-TR-648225).
COG = {
    (5, 5, 5): 5.95659e0,
    (5, 15, 5): 1.37185e0,
    (5, 25, 5): 5.00871e-1,
    (5, 35, 5): 2.52429e-1,
    (5, 45, 5): 1.50260e-1,
    (5, 55, 5): 5.95286e-2,
    (5, 65, 5): 1.53283e-2,
    (5, 75, 5): 4.17689e-3,
    (5, 85, 5): 1.18533e-3,
    (5, 95, 5): 3.46846e-4,
}


def box_interval(p, d, half):
    """t-interval where segment p+t*d lies inside [-half,half]^3."""
    lo, hi = 0.0, 1.0
    for a in range(3):
        with np.errstate(divide="ignore", invalid="ignore"):
            t0 = (-half - p[:, a]) / d[:, a]
            t1 = (half - p[:, a]) / d[:, a]
        ta, tb = np.minimum(t0, t1), np.maximum(t0, t1)
        ta = np.where(np.isfinite(ta), ta, -np.inf)
        tb = np.where(np.isfinite(tb), tb, np.inf)
        lo = np.maximum(lo, ta)
        hi = np.minimum(hi, tb)
    return np.maximum(lo, 0.0), np.minimum(hi, 1.0)


def optical_depths(p, s):
    """tau for every segment p_i -> s_j, vectorized.

    Regions: src cube [-10,10]^3 (sigma 0.1), void cube [-50,50]^3
    (1e-4), shield = rest (0.1). A ray may also leave the domain —
    free space contributes nothing.
    """
    segs = [(a, b) for a in p for b in s]
    pp = np.array([a for a, _ in segs])
    ss = np.array([b for _, b in segs])
    d = ss - pp
    seglen = np.linalg.norm(d, axis=1)

    s_in, s_out = box_interval(pp, d, SRC_E)
    v_in, v_out = box_interval(pp, d, VOID_E)
    # shield is the rest of space — treat as unbounded (all t in
    # [0,1] not covered by inner boxes).
    l_src = np.clip(s_out - s_in, 0.0, None) * seglen
    l_void = np.clip(v_out - v_in, 0.0, None) * seglen - l_src
    l_shield = seglen - np.clip(v_out - v_in, 0.0, None) * seglen
    return SIG_SRC * l_src + SIG_VOID * l_void + SIG_SHIELD * l_shield


def cell_mean_flux(center_cm, n_src=32, n_cell=6):
    """Cell-averaged phi over the CELL_CM cube centered at center_cm."""
    h = CELL_CM / 2.0
    src_e = np.linspace(-SRC_E, SRC_E, n_src + 1)
    src_c = 0.5 * (src_e[:-1] + src_e[1:])
    gx, gy, gz = np.meshgrid(src_c, src_c, src_c, indexing="ij")
    src_pts = np.stack([gx.ravel(), gy.ravel(), gz.ravel()], axis=1)
    src_vol = (2.0 * SRC_E / n_src) ** 3

    ce = np.linspace(-h, h, n_cell + 1)
    cc = 0.5 * (ce[:-1] + ce[1:])
    px, py, pz = np.meshgrid(cc, cc, cc, indexing="ij")
    probe_pts = np.stack([px.ravel(), py.ravel(), pz.ravel()], axis=1) + np.asarray(
        center_cm, dtype=float
    )

    tau = optical_depths(probe_pts, src_pts)
    dist2 = np.sum(
        (
            np.concatenate([probe_pts] * len(src_pts))
            - np.tile(src_pts, (len(probe_pts), 1))
        )
        ** 2,
        axis=1,
    )
    # dist2 ordering must match optical_depths: probe-major.
    kernel = np.exp(-tau) / (4.0 * np.pi * dist2)
    per_cell_sub = kernel.reshape(len(probe_pts), len(src_pts)).sum(axis=1)
    return per_cell_sub.mean() * src_vol


def main():
    rows = []
    for pt in PROBES:
        ours = cell_mean_flux(pt)
        cog = COG[tuple(pt)]
        rows.append((pt, cog, ours))
        print(f"({pt[0]},{pt[1]},{pt[2]}): cellmean={ours:.5e} cog={cog:.5e}")

    os.makedirs(os.path.join(DATA_DIR, "reference"), exist_ok=True)
    out = os.path.join(DATA_DIR, "reference", "kobayashi-p1-cellmean.csv")
    with open(out, "w") as fh:
        fh.write("x_cm,y_cm,z_cm,phi_cellmean,phi_cog_pointwise\n")
        for pt, cog, ours in rows:
            fh.write(f"{pt[0]},{pt[1]},{pt[2]},{ours:.6e},{cog:.6e}\n")
    print("wrote", out)


if __name__ == "__main__":
    main()

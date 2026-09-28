#!/usr/bin/env python3
"""Independent dose-fold check for the FiR-1 cylindrical phantom.

Two audits on one command:

  fold-audit   refold the committed S_N flux artifact through a numpy
               implementation of dose[cell] = sum_g phi_g * R_c,g and
               compare against the committed dose bundle. Must agree
               to ~1e-12 relative — any drift indicts the fold code,
               not the transport.
  mc-fold      fold the same response vectors over the OpenMC MG
               mesh tally (same data file) and compare the dose
               field per component against the committed bundle.
               Differences are transport-model differences carried
               through the dose pipeline.

Usage:
    dose_fold_check.py <statepoint.h5> [--dose dose.json] [--flux flux.json]
"""
import argparse
import json
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
PHANTOM = ROOT / "validation/fir1-k63-cylindrical-phantom"
NX, NY, NZ = 24, 24, 26
V_CELL = 1.0  # cm^3


def load_fields(dose_path: Path, flux_path: Path, sp_path: Path | None):
    dose = json.loads(dose_path.read_text())
    flux = json.loads(flux_path.read_text())
    data = json.loads((PHANTOM / "multigroup-data-28g-tsl-v4.json").read_text())
    assign = json.loads((PHANTOM / "assignment.json").read_text())

    n_cells = NX * NY * NZ
    mask = np.zeros(n_cells, dtype=bool)
    for ijk in assign["regions"][0]["shape"]["indices"]:
        mask[ijk[0] + NX * ijk[1] + NX * NY * ijk[2]] = True

    # material id per cell (void unless masked)
    water_id, void_id = (m["material_id"] for m in data["materials"])
    mat_id = np.where(mask, water_id, void_id)
    resp = {
        name: {c: np.array(m["dose_response_gy_cm2"][c]) for c in m["dose_response_gy_cm2"]}
        for name, m in ((mm["material_id"], mm) for mm in data["materials"])
    }

    sn = np.array(flux["flux"])
    components = {
        c["component"]: np.array(c["values"]) for c in dose["components"]
    }
    committed_total = np.array(dose["physical_total"]["values"])

    mc = None
    if sp_path is not None:
        import openmc

        with openmc.StatePoint(str(sp_path)) as sp:
            t = sp.get_tally(name="cell_flux")
            mean = t.mean.reshape(n_cells, -1)
        mc = mean[:, ::-1] / V_CELL  # ascending bins -> our group order

    return components, committed_total, sn, mc, mat_id, resp, mask


def fold(flux, mat_id, resp, names, n_cells):
    """dose_c[cell] = sum_g phi[cell,g] * R_c[mat][g]."""
    out = {}
    for c in names:
        d = np.zeros(n_cells)
        for cell in range(n_cells):
            d[cell] = float(flux[cell] @ resp[mat_id[cell]][c])
        out[c] = d
    return out


def describe(name, a, b, mask):
    ok = mask & (np.abs(b) > 0)
    r = a[ok] / b[ok]
    print(f"{name:<10} n={ok.sum():<5} median={np.median(r):.4f} "
          f"p16={np.percentile(r,16):.4f} p84={np.percentile(r,84):.4f} "
          f"max|rel|={np.max(np.abs(r-1)):.3e}")
    return {"n": int(ok.sum()), "median": float(np.median(r)),
            "p16": float(np.percentile(r, 16)), "p84": float(np.percentile(r, 84)),
            "max_abs_rel": float(np.max(np.abs(r - 1)))}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("statepoint", nargs="?")
    ap.add_argument("--dose", default=str(PHANTOM / "dose-28g-tsl-p1-1e.json"))
    ap.add_argument("--flux", default=str(PHANTOM / "multigroup-flux-28g-tsl-p1-1e.json"))
    ap.add_argument("--out")
    args = ap.parse_args()

    components, committed, sn, mc, mat_id, resp, mask = load_fields(
        Path(args.dose), Path(args.flux),
        Path(args.statepoint) if args.statepoint else None)

    n_cells = NX * NY * NZ
    names = sorted(resp[mat_id[0]].keys())
    report = {"fold_audit": {}, "mc_fold": {}}

    print("== fold-audit: refold(SN flux) vs committed dose bundle ==")
    refolded = fold(sn, mat_id, resp, names, n_cells)
    refolded_total = np.zeros(n_cells)
    for c in names:
        refolded_total += refolded[c]
        report["fold_audit"][c] = describe(c, refolded[c], components[c], mask)
    report["fold_audit"]["physical_total"] = describe(
        "total", refolded_total, committed, mask)

    if mc is not None:
        print("\n== mc-fold: fold(MC flux) vs committed dose bundle ==")
        mcfold = fold(mc, mat_id, resp, names, n_cells)
        mc_total = np.zeros(n_cells)
        for c in names:
            mc_total += mcfold[c]
            report["mc_fold"][c] = describe(c, mcfold[c], components[c], mask)
        report["mc_fold"]["physical_total"] = describe(
            "total", mc_total, committed, mask)

        # depth profile of total dose ratio on the beam axis (cell 11-12 x ~11-12)
        print("\nz-depth axis profile (axis cells, water-masked):")
        axis = [i for i in range(n_cells)
                if mask[i] and i % NX in (11, 12) and (i // NX) % NY in (11, 12)]
        axis.sort(key=lambda c: c // (NX * NY))
        axis = [c for c in axis if c % NX == 11 and (c // NX) % NY == 11]
        axis.sort(key=lambda c: c // (NX * NY))
        for c in axis[:24]:
            z = c // (NX * NY)
            print(f"  z={z:>2}  SN={committed[c]:.3e}  MC={mc_total[c]:.3e}  "
                  f"ratio={committed[c]/mc_total[c]:.3f}")

    if args.out:
        Path(args.out).write_text(json.dumps(report, indent=1))
        print("wrote", args.out)
    return 0


if __name__ == "__main__":
    sys.exit(main())

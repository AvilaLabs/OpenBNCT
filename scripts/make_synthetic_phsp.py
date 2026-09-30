#!/usr/bin/env python3
"""Sample an IAEA phase-space file from an analytic OpenBNCT beam description.

Reproduces the analytic `uniform_disk` + `isotropic_cone` + tabulated-histogram
source (a beam description JSON such as beams/fir1-k63-ineel-20mev.json) as
neutron records in the IAEA phase-space format, so the phase-space path can be
validated against the analytic one on identical physics:

    python3 scripts/make_synthetic_phsp.py --beam beams/fir1-k63-ineel-20mev.json \
        --n 10000000 --seed 20260930 --out /path/to/fir1-synthetic

writes fir1-synthetic.IAEAheader / fir1-synthetic.IAEAphsp. Records are
little-endian, 25 bytes: type (int8, negative = new history; every neutron
starts its own history), energy MeV (float32, sign = sign of W), X, Y (cm,
relative to the port axis), U, V, weight (float32); Z is the header constant 0
and W is derived from U and V. Energy is uniform per eV inside each histogram
bin, the OpenMC/MCNP convention (and `SourceWeighting::UniformInBin`).
Nothing here is large enough to commit: keep the outputs out of the tree.
"""
import argparse
import json
import math
import sys

import numpy as np

RECORD = np.dtype(
    [("t", "i1"), ("e", "<f4"), ("x", "<f4"), ("y", "<f4"), ("u", "<f4"), ("v", "<f4"), ("wt", "<f4")]
)
assert RECORD.itemsize == 25


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--beam", required=True, help="openbnct.beam-description JSON")
    ap.add_argument("--n", type=int, required=True, help="neutrons to sample")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--out", required=True, help="output stem (no extension)")
    ap.add_argument("--chunk", type=int, default=1_000_000)
    args = ap.parse_args()

    beam = json.load(open(args.beam))
    src = beam["source"]
    space, angle, energy = src["space"], src["angle"], src["energy"]
    if space["kind"] != "uniform_disk" or angle["kind"] != "isotropic_cone":
        sys.exit("only uniform_disk + isotropic_cone beams are supported")
    if energy["kind"] != "tabulated_histogram":
        sys.exit("only tabulated_histogram spectra are supported")
    radius = float(space["radius_cm"])
    cos0 = math.cos(float(angle["half_angle_rad"]))
    edges = np.asarray(energy["energy_boundaries_ev"], dtype=np.float64)
    weights = np.asarray(energy["bin_weights"], dtype=np.float64)
    probs = weights / weights.sum()

    rng = np.random.default_rng(args.seed)
    with open(args.out + ".IAEAphsp", "wb") as fh:
        remaining = args.n
        while remaining > 0:
            m = min(args.chunk, remaining)
            remaining -= m
            r = radius * np.sqrt(rng.random(m))
            phi_pos = 2 * np.pi * rng.random(m)
            mu = cos0 + (1.0 - cos0) * rng.random(m)
            phi_dir = 2 * np.pi * rng.random(m)
            sin = np.sqrt(np.maximum(0.0, 1.0 - mu * mu))
            bins = rng.choice(len(probs), size=m, p=probs)
            e_ev = edges[bins] + (edges[bins + 1] - edges[bins]) * rng.random(m)
            rec = np.empty(m, dtype=RECORD)
            rec["t"] = -4
            rec["e"] = e_ev * 1e-6
            rec["x"] = r * np.cos(phi_pos)
            rec["y"] = r * np.sin(phi_pos)
            rec["u"] = sin * np.cos(phi_dir)
            rec["v"] = sin * np.sin(phi_dir)
            rec["wt"] = 1.0
            fh.write(rec.tobytes())

    header = f"""$IAEA_INDEX:
0

$TITLE:
Synthetic neutron phase space sampled from {beam.get('id', 'a beam description')} (seed {args.seed})

$FILE_TYPE:
0

$RECORD_CONTENTS:
     1     // X is stored ?
     1     // Y is stored ?
     0     // Z is stored ?
     1     // U is stored ?
     1     // V is stored ?
     1     // W is stored ?
     1     // Weight is stored ?
     0     // Extra floats stored ?
     0     // Extra longs stored ?

$RECORD_CONSTANT:
     0.0   // Constant X
     0.0   // Constant Y
     0.0   // Constant Z
     0.0   // Constant U
     0.0   // Constant V
     0.0   // Constant W
     1.0   // Constant Weight

$RECORD_LENGTH:
{RECORD.itemsize}

$BYTE_ORDER:
1234

$ORIGINAL_HISTORIES:
{args.n}

$PARTICLES:
{args.n}

$PHOTONS:
0

$ELECTRONS:
0

$POSITRONS:
0

$NEUTRONS:
{args.n}

$PROTONS:
0

$MONTE_CARLO_CODE_VERSION:
scripts/make_synthetic_phsp.py (numpy default_rng, seed {args.seed})
"""
    with open(args.out + ".IAEAheader", "w") as fh:
        fh.write(header)
    print(f"wrote {args.n} neutrons to {args.out}.IAEAphsp ({args.n * RECORD.itemsize / 1e6:.1f} MB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

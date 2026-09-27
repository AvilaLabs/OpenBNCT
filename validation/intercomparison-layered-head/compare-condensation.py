#!/usr/bin/env python3
"""Condensation-weighting A/B for the layered-head phantom.

Compares group-collapsed S_N flux fields against the continuous-energy
OpenMC reference on the beam axis, per MC energy bin:
  mc   — openmc-tallies.json continuous-energy reference
  args — one or more multigroup-flux artifacts to compare

Usage: compare-condensation.py FLUX.json [FLUX2.json ...]
"""
import json
import sys

ROOT = "benchmarks/synthetic/layered-head-phantom"
mc = json.load(open("validation/intercomparison-layered-head/openmc-tallies.json"))
mbins = [float(x) for x in mc["energy_bins_ev"]]  # ascending
NB = len(mbins) - 1

nx, ny = 25, 25
ax = [12 + nx * 12 + nx * ny * k for k in range(25)]

def bin_flux(flux, eb, cell):
    return [
        sum(
            flux[cell][g]
            for g in range(len(eb) - 1)
            if eb[g] <= mbins[b + 1] and eb[g + 1] >= mbins[b]
        )
        for b in range(NB)
    ]

docs = []
for path in sys.argv[1:]:
    art = json.load(open(path))
    eb = [float(x) for x in art["energy_boundaries_ev"]]
    docs.append((path.split("/")[-1], art, eb))

print(f"{'z(cm)':>6} {'mc_therm':>10}", end="")
for name, _, _ in docs:
    print(f" {name[:22]:>22} {'r':>6}", end="")
print("  |  bin rows: 0=therm 2=epi 4=fast")

for k in [3, 6, 9, 12, 15, 18, 21]:
    c = ax[k]
    z = 0.8 * k + 0.4
    mrow = mc["flux_mean"][c * NB : (c + 1) * NB]
    print(f"{z:6.1f}")
    for b, label in [(0, "therm"), (2, "epi"), (4, "fast")]:
        print(f"  {label:>5} mc={mrow[b]:10.4e}", end="")
        for name, art, eb in docs:
            bf = bin_flux(art["flux"], eb, c)[b]
            print(f" {bf:10.4e} {bf/mrow[b] if mrow[b] else float('nan'):6.3f}", end="")
        print()
    for name, art, _ in docs:
        print(f"  audit({name[:40]}): {art.get('balance_absorbed_fraction')}", end="")
    print()

#!/usr/bin/env python3
"""A/B check for the theta-repair sweep on the layered-head phantom.

Compares three flux fields on the beam axis:
  legacy    — committed multigroup-flux-56g-shielded.json (clamp path,
              known to absorb ~42x its source)
  repaired  — fresh multigroup-flux-56g-thetarepair.json (step-closure
              fallback, expected to conserve)
  mc        — openmc-tallies.json continuous-energy reference

Reports the global balance audit from each artifact plus the axial
total-flux profile ratios.
"""
import json
import sys

ROOT = "benchmarks/synthetic/layered-head-phantom"
legacy = json.load(open(f"{ROOT}/multigroup-flux-56g-shielded.json"))
repaired = json.load(open(f"{ROOT}/multigroup-flux-56g-thetarepair.json"))
mc = json.load(open("validation/intercomparison-layered-head/openmc-tallies.json"))

for name, art in [("legacy", legacy), ("repaired", repaired)]:
    print(
        f"{name}: converged={art['converged']} residual={art['residual']:.3e} "
        f"outer={art['outer_iterations']} "
        f"balance_absorbed_fraction={art.get('balance_absorbed_fraction')}"
    )

# Axis profile: the phantom is 25x25x25 voxels of 8 mm, beam enters the
# -z face at the central column (x=y=12). MC tallies are flat
# [cell*5 + bin] over energy_bins_ev.
nx, ny = 25, 25
ax = [12 + nx * 12 + nx * ny * k for k in range(25)]
leg_tot = [sum(legacy["flux"][i]) for i in ax]
rep_tot = [sum(repaired["flux"][i]) for i in ax]
mc_tot = [sum(mc["flux_mean"][i * 5 : i * 5 + 5]) for i in ax]

print(f"\n{'z(mm)':>6} {'MC':>11} {'legacy':>11} {'repaired':>11} "
      f"{'rep/leg':>8} {'rep/MC':>8}")
for k, i in enumerate(ax):
    z = 8 * k + 4
    l, r, m = leg_tot[k], rep_tot[k], mc_tot[k]
    print(f"{z:6.0f} {m:11.4e} {l:11.4e} {r:11.4e} {r/l:8.3f} {r/m:8.3f}")

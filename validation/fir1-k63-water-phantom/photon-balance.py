#!/usr/bin/env python3
"""Photon transport balance ledger for the FIR1-K63 water phantom.

Reconstructs the per-group steady-state balance of the committed
`sn photon-solve` result directly from the committed artifacts:

    leak_g  =  production_g  +  scatter-in_g  -  sigma_t,g * phi_g

summed over all cells (voxel volume cancels in the totals). The
identity is exact for the discrete system: every produced or
in-scattered photon is either removed by sigma_t or leaks out of a
boundary face. Energy weighting uses the group geometric midpoints —
the only approximation in the ledger (~few-percent level).

It also folds the energy deposition split:

    deposit_g  =  phi_g * Sigma_gp (E_g - E_gp) * sigma_s(g->gp)   (Compton recoil)
               +  phi_g * sigma_a,g * E_g                        (photoelectric/pair)

Usage:  python3 photon-balance.py            # committed S8 flux
        python3 photon-balance.py FILE.json  # any photon flux artifact

Companion oracle: photon-escape-mc.py (independent Klein-Nishina
estimate of the true escape fraction for this geometry).
"""
import json
import sys

HERE = __file__.rsplit("/", 1)[0]

def load(name):
    with open(f"{HERE}/{name}") as f:
        return json.load(f)

flux_path = sys.argv[1] if len(sys.argv) > 1 else f"{HERE}/multigroup-photon-flux-16g.json"
data = load("multigroup-photon-data-16g.json")
case = load("case.json")
nflux = load("multigroup-flux-28g-tsl-p1-1e.json")
with open(flux_path) as f:
    pflux = json.load(f)

edges = data["energy_boundaries_ev"]
G = len(edges) - 1
Gn = len(data["neutron_energy_boundaries_ev"]) - 1
E_mid = [(edges[g] * edges[g + 1]) ** 0.5 for g in range(G)]

m = data["materials"][0]
st = m["sigma_total_per_cm"]
sm = m["scatter_matrix_per_cm"]
pm = m["production_matrix_per_cm"]

ncell = len(pflux["flux"])
phi = [sum(pflux["flux"][c][g] for c in range(ncell)) for g in range(G)]

prod = [0.0] * G
for c in range(ncell):
    row = nflux["flux"][c]
    for g in range(G):
        prod[g] += sum(row[gn] * pm[gn * G + g] for gn in range(Gn))

scin = [0.0] * G
dep_scat = [0.0] * G
dep_abs = [0.0] * G
for g in range(G):
    sa = st[g]
    for gp in range(G):
        s = sm[g * G + gp]
        sa -= s
        scin[gp] += s * phi[g]
        dep_scat[g] += (E_mid[g] - E_mid[gp]) * s * phi[g]
    dep_abs[g] = sa * E_mid[g] * phi[g]

leak = [prod[g] + scin[g] - st[g] * phi[g] for g in range(G)]
e_prod = sum(prod[g] * E_mid[g] for g in range(G))
e_leak = sum(leak[g] * E_mid[g] for g in range(G))
e_dep = sum(dep_scat) + sum(dep_abs)
n_prod = sum(prod)
n_leak = sum(leak)

print(f"flux: {flux_path}")
print(f"cells: {ncell}   photon groups: {G}   neutron groups: {Gn}")
print()
print(" grp |    E range keV |    prod    |  scat-in   |  removal   |  leak (implied)")
for g in range(G):
    print(
        f" g{g:2d} | {edges[g+1]/1e3:7.0f}..{edges[g]/1e3:7.0f} |"
        f" {prod[g]:10.3e} | {scin[g]:10.3e} | {st[g]*phi[g]:10.3e} | {leak[g]:10.3e}"
    )
print()
print(f"number:  produced {n_prod:.4e}   leaked {n_leak:.4e} ({n_leak/n_prod*100:.1f}%)")
print(f"energy:  produced {e_prod:.4e}   leaked {e_leak:.4e} ({e_leak/e_prod*100:.1f}%)")
print(f"         deposited {e_dep:.4e} ({e_dep/e_prod*100:.1f}%)   "
      f"closure {(e_leak+e_dep)/e_prod*100:.1f}%")

# Boundary-cell flux proxy per face (not a current — needs angular flux,
# but shows which faces carry the field).
nx, ny, nz = case["geometry"]["shape"]
faces = dict.fromkeys(("x0", "x1", "y0", "y1", "z0", "z1"), 0.0)
for c in range(ncell):
    i, j, k = c % nx, (c // nx) % ny, c // (nx * ny)
    f = sum(pflux["flux"][c])
    if i == 0:
        faces["x0"] += f
    if i == nx - 1:
        faces["x1"] += f
    if j == 0:
        faces["y0"] += f
    if j == ny - 1:
        faces["y1"] += f
    if k == 0:
        faces["z0"] += f
    if k == nz - 1:
        faces["z1"] += f
print("boundary-cell total flux by face:",
      {k: f"{v:.3e}" for k, v in faces.items()})

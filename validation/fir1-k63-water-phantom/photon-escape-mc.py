#!/usr/bin/env python3
"""Independent Monte Carlo estimate of the photon escape fraction.

Analog Klein-Nishina cascade on continuous energy, sampled over the
*committed* production map (n->gamma production from the committed
neutron flux, essentially all in the 2-3 MeV bin -> monoenergetic
2.223 MeV source). Cross sections are a log-log interpolant of the
NIST XCOM water table below — deliberately independent of the
collapsed multigroup data, so this oracle cannot inherit a collapse
bias. Compton recoil energy deposits locally; photoelectric + pair
events absorb; a photon that leaves the 52x52x47 cm water box has
escaped.

This is the check that reframed R11-02: the committed README had
inferred "~15% MC boundary escape" by assuming the OpenMC photon
tally deposits ~85% of produced energy. For a 22.6 cm mean-free-path
2.2 MeV source concentrated in the first ~10 cm of a 47 cm box,
the true escape fraction is ~60% of energy — the inference was
wrong, not the transport.

Usage:  python3 photon-escape-mc.py [N]
"""
import bisect
import json
import math
import random
import sys

HERE = __file__.rsplit("/", 1)[0]

def load(name):
    with open(f"{HERE}/{name}") as f:
        return json.load(f)

data = load("multigroup-photon-data-16g.json")
case = load("case.json")
nflux = load("multigroup-flux-28g-tsl-p1-1e.json")

m = data["materials"][0]
G = len(data["energy_boundaries_ev"]) - 1
Gn = len(data["neutron_energy_boundaries_ev"]) - 1
pm = m["production_matrix_per_cm"]

nx, ny, nz = case["geometry"]["shape"]
sp = [s / 10.0 for s in case["geometry"]["spacing_mm"]]
LX, LY, LZ = nx * sp[0], ny * sp[1], nz * sp[2]

prod_cell = [
    sum(
        nflux["flux"][c][gn] * sum(pm[gn * G:(gn + 1) * G])
        for gn in range(Gn)
    )
    for c in range(nx * ny * nz)
]
total_prod = sum(prod_cell)
cum = [0.0] * len(prod_cell)
s = 0.0
for i, p in enumerate(prod_cell):
    s += p
    cum[i] = s

# NIST XCOM water, rho = 1 g/cm3: total and incoherent (Compton)
# mass attenuation coefficients (cm2/g). Energies in MeV.
E_TAB = [0.001, 0.002, 0.005, 0.01, 0.02, 0.03, 0.05, 0.1, 0.2, 0.3,
         0.4, 0.5, 0.6, 0.8, 1.0, 1.5, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0]
MU_TOT = [5.329, 3.713, 1.235, 0.5329, 0.8095, 0.3757, 0.2266,
          0.1707, 0.1370, 0.1186, 0.1061, 0.0966, 0.0896,
          0.0786, 0.0708, 0.0576, 0.0493, 0.0396, 0.0339, 0.0303,
          0.0277, 0.0250, 0.0229]
MU_COMP = [0.0000, 0.0050, 0.1815, 0.4208, 0.2759, 0.2091, 0.1880,
           0.1547, 0.1303, 0.1121, 0.1006, 0.0929, 0.0857, 0.0746,
           0.0667, 0.0554, 0.0472, 0.0377, 0.0318, 0.0279, 0.0251,
           0.0214, 0.0190]

def interp(tab, e):
    if e <= E_TAB[0]:
        return tab[0]
    if e >= E_TAB[-1]:
        return tab[-1]
    for i in range(len(E_TAB) - 1):
        if E_TAB[i] <= e <= E_TAB[i + 1]:
            t = math.log(e / E_TAB[i]) / math.log(E_TAB[i + 1] / E_TAB[i])
            return math.exp(math.log(tab[i]) * (1 - t)
                            + math.log(tab[i + 1]) * t)
    return tab[-1]

def sigma_t(e):
    return interp(MU_TOT, e)

def abs_frac(e):
    return max(0.0, 1.0 - interp(MU_COMP, e) / sigma_t(e))

def kn_scatter(e):
    """Kahn-style rejection on the Klein-Nishina energy ratio."""
    eps_min = 1.0 / (1.0 + 2.0 * e / 0.511)
    while True:
        eps = eps_min + (1.0 - eps_min) * random.random()
        if random.random() < (eps + 1.0 / eps) / 2.0:
            ep = e * eps
            mu = 1.0 - 0.511 * (1.0 / ep - 1.0 / e)
            return ep, max(-1.0, min(1.0, mu))

def scatter_dir(d, mu):
    ct = mu
    st = math.sqrt(max(0.0, 1.0 - ct * ct))
    ph = random.uniform(0.0, 2.0 * math.pi)
    ux, uy, uz = d
    n = math.sqrt(ux * ux + uy * uy)
    if n < 1e-9:
        return (st * math.cos(ph), st * math.sin(ph), ct)
    vx, vy = -uy / n, ux / n
    wx, wy, wz = uz * vx, uz * vy, -(ux * vx + uy * vy)
    ax = ct * ux + st * (math.cos(ph) * vx + math.sin(ph) * wx)
    ay = ct * uy + st * (math.cos(ph) * vy + math.sin(ph) * wy)
    az = ct * uz + st * math.sin(ph) * wz
    nn = math.sqrt(ax * ax + ay * ay + az * az)
    return (ax / nn, ay / nn, az / nn)

def run(n):
    esc = [0] * 6
    esc_e = [0.0] * 6
    dep_e = 0.0
    for _ in range(n):
        c = bisect.bisect_left(cum, random.random() * total_prod)
        i, j, k = c % nx, (c // nx) % ny, c // (nx * ny)
        x = (i + random.random()) * sp[0]
        y = (j + random.random()) * sp[1]
        z = (k + random.random()) * sp[2]
        e = 2.223  # H(n,gamma) capture energy; >99% of production is in g2
        u = random.uniform(-1.0, 1.0)
        th = random.uniform(0.0, 2.0 * math.pi)
        sy = math.sqrt(1.0 - u * u)
        d = (sy * math.cos(th), sy * math.sin(th), u)
        while True:
            s = -math.log(1.0 - random.random()) / sigma_t(e)
            x += d[0] * s
            y += d[1] * s
            z += d[2] * s
            if not (0.0 < x < LX and 0.0 < y < LY and 0.0 < z < LZ):
                f = (0 if x <= 0 else 1 if x >= LX else
                     2 if y <= 0 else 3 if y >= LY else
                     4 if z <= 0 else 5)
                esc[f] += 1
                esc_e[f] += e
                break
            if random.random() < abs_frac(e):
                dep_e += e
                break
            ep, mu = kn_scatter(e)
            dep_e += e - ep
            e = ep
            if e < 0.005:
                dep_e += e
                break
            d = scatter_dir(d, mu)
    tot_e = dep_e + sum(esc_e)
    print(f"N = {n}")
    print("escape by face (energy share):",
          {f: f"{esc_e[i] / tot_e * 100:.1f}%"
           for i, f in enumerate("x0 x1 y0 y1 z0 z1".split())})
    print(f"energy:  escaped {sum(esc_e) / tot_e * 100:.1f}%   "
          f"deposited {dep_e / tot_e * 100:.1f}%")
    print(f"number:  escaped {sum(esc) / n * 100:.1f}%")

if __name__ == "__main__":
    random.seed(7)
    run(int(sys.argv[1]) if len(sys.argv) > 1 else 40000)

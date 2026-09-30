#!/usr/bin/env python3
"""Shared helpers for the S_N-vs-continuous-energy regression references.

Imported by each case's generate_reference.py. Everything here is
deterministic: the cross_sections.xml copy is rebuilt from the library
index with absolute paths, the S(alpha,beta) table is attached to H, and
identity hashes are recorded for the data-identity.json of every case.

Run the case scripts with the OpenMC 0.16.0 interpreter, e.g.
  OMP_NUM_THREADS=3 ~/micromamba/envs/openmc016/bin/python generate_reference.py
"""

import hashlib
import json
import os
import re
import tempfile

import numpy as np
import openmc
import sys

# The `openmc` executable lives next to the interpreter of its environment.
os.environ["PATH"] = os.path.dirname(sys.executable) + os.pathsep + os.environ.get("PATH", "")

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
BENCH = os.path.join(REPO, "benchmarks", "synthetic", "layered-head-phantom")
SN_DATA = os.path.join(BENCH, "multigroup-data-28g-v5-tsl.json")

LIBRARY_DIR = os.environ.get(
    "OPENBNCT_ENDFB81_DIR",
    "/home/connoravila/Documents/Avila-Labs/.nctforge-data/nctforge/"
    "openmc-endfb81-official-library/selected/endfb-viii.1-hdf5",
)
TSL_H5 = os.environ.get(
    "OPENBNCT_TSL_H2O",
    "/home/connoravila/nuclear-data/endfb-viii.1-hdf5/thermal/c_H_in_H2O.h5",
)
TSL_NAME = "c_H_in_H2O"
TEMPERATURE_K = 293.6

# Source spectrum shared by every case (uniform per eV inside each bin).
SRC_BOUNDS_EV = [1.0e-5, 0.5, 1.0e4, 1.69e7]
SRC_PROBS = [0.0611, 0.9093, 0.0296]

# Coarse reporting bands on S_N group boundaries.
THERMAL_MAX_EV = 0.5
EPITHERMAL_MAX_EV = 1.0e4


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def sn_group_bounds_ascending():
    """The 29 S_N group boundaries (the data stores them descending)."""
    d = json.load(open(SN_DATA))
    return sorted(d["energy_boundaries_ev"])


def band_of_group_ascending(bounds_asc):
    """Band index (0 thermal, 1 epithermal, 2 fast) of each ascending group."""
    out = []
    for hi in bounds_asc[1:]:
        if hi <= THERMAL_MAX_EV * (1 + 1e-9):
            out.append(0)
        elif hi <= EPITHERMAL_MAX_EV * (1 + 1e-9):
            out.append(1)
        else:
            out.append(2)
    return out


def build_cross_sections(outdir):
    """Write a cross_sections.xml with absolute neutron paths and the
    c_H_in_H2O thermal table appended. Neutron entries keep the library
    order, so the file is byte-deterministic for a given library."""
    src = os.path.join(LIBRARY_DIR, "cross_sections.xml")
    lines = ["<?xml version='1.0' encoding='UTF-8'?>", "<cross_sections>"]
    for line in open(src):
        m = re.search(
            r'<library materials="([^"]+)" path="(neutron/[^"]+)" type="neutron"/>', line
        )
        if m:
            path = os.path.join(LIBRARY_DIR, m.group(2))
            lines.append(f'  <library materials="{m.group(1)}" path="{path}" type="neutron"/>')
    lines.append(f'  <library materials="{TSL_NAME}" path="{TSL_H5}" type="thermal"/>')
    lines.append("</cross_sections>")
    path = os.path.join(outdir, "cross_sections.xml")
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")
    return path


def load_material(name):
    d = json.load(open(os.path.join(BENCH, "materials", f"{name}.json")))
    m = openmc.Material(name=name)
    for n in d["nuclides"]:
        m.add_nuclide(n["name"], n["mass_fraction"], "wo")
    m.set_density("g/cm3", d["density_g_cm3"])
    m.temperature = d["temperature_k"]
    if any(n["name"] == "H1" for n in d["nuclides"]):
        m.add_s_alpha_beta(TSL_NAME)
    return m, d


def source_spectrum():
    """Histogram spectrum. OpenMC 0.16 Tabular(histogram) takes a pdf
    DENSITY per eV, so each bin probability is divided by its width."""
    dens = [p / (SRC_BOUNDS_EV[i + 1] - SRC_BOUNDS_EV[i]) for i, p in enumerate(SRC_PROBS)]
    return openmc.stats.Tabular(SRC_BOUNDS_EV, dens, interpolation="histogram")


def check_spectrum_normalization():
    """Bin probabilities recovered from the density must equal SRC_PROBS."""
    sp = source_spectrum()
    x, p = np.asarray(sp.x), np.asarray(sp.p)
    probs = p * np.diff(x)
    assert np.allclose(probs, SRC_PROBS, rtol=1e-6), probs


def settings(particles, batches, seed, cross_sections):
    s = openmc.Settings()
    s.run_mode = "fixed source"
    s.particles = particles
    s.batches = batches
    s.inactive = 0
    s.seed = seed
    s.temperature = {"method": "nearest", "tolerance": 10.0}
    return s


def workdir(tag):
    return tempfile.mkdtemp(prefix=f"openbnct-reg-{tag}-")


def write_identity(case_dir, tag, cross_sections, extra):
    ident = {
        "schema_version": "openbnct.regression-data-identity/0.1.0",
        "case": tag,
        "openmc_version": openmc.__version__,
        "neutron_library_dir": LIBRARY_DIR,
        "library_cross_sections_xml_sha256": sha256(
            os.path.join(LIBRARY_DIR, "cross_sections.xml")
        ),
        "generated_cross_sections_xml_sha256": sha256(cross_sections),
        "sab_table": TSL_NAME,
        "sab_file": TSL_H5,
        "sab_file_sha256": sha256(TSL_H5),
        "material_temperature_k": TEMPERATURE_K,
        "sn_multigroup_data": os.path.relpath(SN_DATA, REPO),
        "sn_multigroup_data_sha256": sha256(SN_DATA),
    }
    ident.update(extra)
    with open(os.path.join(case_dir, "data-identity.json"), "w") as f:
        json.dump(ident, f, indent=1)
        f.write("\n")


def materials(mat_list, cross_sections):
    """Materials collection pointing at the generated cross_sections.xml."""
    ms = openmc.Materials(mat_list)
    ms.cross_sections = cross_sections
    return ms


def run_model(model, work):
    """Run OpenMC (OMP threads from OMP_NUM_THREADS, default 3) and return
    the statepoint path. OPENBNCT_REG_REUSE_DIR=<dir> re-reads an existing
    statepoint instead (post-processing only)."""
    reuse = os.environ.get("OPENBNCT_REG_REUSE_DIR")
    if reuse:
        hits = sorted(f for f in os.listdir(reuse) if f.startswith("statepoint"))
        return os.path.join(reuse, hits[-1])
    return model.run(cwd=work, threads=int(os.environ.get("OMP_NUM_THREADS", "3")))


def macroscopic_total_xs(material, energies_ev):
    """Sigma_t [1/cm] of an openmc.Material at the given energies, from the
    294 K total (MT 1) cross sections of the neutron library."""
    from openmc.data import IncidentNeutron

    sigma = np.zeros_like(energies_ev, dtype=float)
    for name, dens in material.get_nuclide_atom_densities().items():
        nuc = IncidentNeutron.from_hdf5(os.path.join(LIBRARY_DIR, "neutron", f"{name}.h5"))
        sigma += dens * nuc[1].xs["294K"](energies_ev)  # atoms/b-cm * b = 1/cm
    return sigma


def fmt(a, sig=6):
    """Round an array to `sig` significant digits (keeps files small)."""
    a = np.asarray(a, dtype=float)
    return [float(f"{x:.{sig}g}") for x in a.ravel()]

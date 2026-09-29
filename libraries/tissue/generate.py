#!/usr/bin/env python3
"""Regenerate the standard tissue library material and HU-calibration files.

Deterministic: same inputs -> byte-identical output. Inputs are the element
weight fractions printed in PNNL-15870 Rev. 1 (transcribed below, one row per
printed table line) and natural isotopic abundances / isotopic masses.

Expansion rule (element -> nuclide):
  1. split each element into the nuclides for which the repo's local
     ENDF/B-VIII.1 data exist, using natural atom abundances converted to
     mass fractions with isotopic masses (whole natural element mass is the
     denominator, so dropped isotopes carry their true mass share);
  2. any element or isotope with NO local evaluation (Ar, Si, Zn, Fe54/57/58,
     S36) is dropped -- never substituted -- and the remaining mass fractions
     are renormalized to sum to one. The dropped mass fraction and the
     renormalization factor are recorded in audit.json.
"""
import json
import math
import os

HERE = os.path.dirname(os.path.abspath(__file__))

# element -> (kept isotopes [(name, atom abundance, mass u)], lost isotopes [(abundance, mass u)])
ISO = {
    "H": ([("H1", 0.999885, 1.00782503), ("H2", 0.000115, 2.01410178)], []),
    "C": ([("C12", 0.9893, 12.0), ("C13", 0.0107, 13.00335484)], []),
    "N": ([("N14", 0.99636, 14.00307401), ("N15", 0.00364, 15.00010890)], []),
    "O": ([("O16", 0.99757, 15.99491462), ("O17", 0.00038, 16.99913176), ("O18", 0.00205, 17.99915961)], []),
    "Na": ([("Na23", 1.0, 22.98976928)], []),
    "Mg": ([("Mg24", 0.7899, 23.98504170), ("Mg25", 0.1000, 24.98583698), ("Mg26", 0.1101, 25.98259297)], []),
    "P": ([("P31", 1.0, 30.97376200)], []),
    "S": ([("S32", 0.9499, 31.97207117), ("S33", 0.0075, 32.97145891), ("S34", 0.0425, 33.96786701)],
           [(0.0001, 35.96708076)]),
    "Cl": ([("Cl35", 0.7576, 34.96885268), ("Cl37", 0.2424, 36.96590259)], []),
    "K": ([("K39", 0.932581, 38.96370668), ("K40", 0.000117, 39.96399848), ("K41", 0.067302, 40.96182576)], []),
    "Ca": ([("Ca40", 0.96941, 39.96259098), ("Ca42", 0.00647, 41.95861801), ("Ca43", 0.00135, 42.9587666),
            ("Ca44", 0.02086, 43.9554818), ("Ca46", 0.00004, 45.9536926), ("Ca48", 0.00187, 47.952534)], []),
    # Only Fe56 has a local evaluation; Fe54/57/58 are dropped.
    "Fe": ([("Fe56", 0.91754, 55.9349375)],
           [(0.05845, 53.9396084), (0.02119, 56.9353928), (0.00282, 57.9332744)]),
}
UNSUPPORTED = {"Ar", "Si", "Zn"}
T_K = 293.6
ELEM_ORDER = ["H", "C", "N", "O", "Na", "Mg", "P", "S", "Cl", "K", "Ca", "Fe"]

PNNL = ("PNNL-15870 Rev. 1, Compendium of Material Composition Data for Radiation "
        "Transport Modeling (PIET-43741-TM-963)")

# id -> (printed name, entry number, printed page ('Page N of 357'), density g/cm3, source comment, printed element weight fractions)
M = {
    "air-dry": ("Air (Dry, Near Sea Level)", 4, 18, 0.001205,
                "NIST 1996 (physics.nist.gov/PhysRefData/XrayMassCoef/tab2.html)",
                {"C": 0.000124, "N": 0.755268, "O": 0.231781, "Ar": 0.012827}),
    "water-liquid": ("Water, Liquid", 354, 338, 0.998207,
                     "NIST 1998 (compos.pl matno=276); density 0.998207 g/cm3, de-aerated SMOW at 1 atm (Tanaka et al. 2001)",
                     {"H": 0.111894, "O": 0.888106}),
    "adipose-icrp": ("Tissue, Adipose (ICRP)", 316, 302, 0.92, "NIST 1998 (compos.pl matno=103)",
                     {"H": 0.119477, "C": 0.637240, "N": 0.007970, "O": 0.232333, "Na": 0.000500, "Mg": 0.000020,
                      "P": 0.000160, "S": 0.000730, "Cl": 0.001190, "K": 0.000320, "Ca": 0.000020, "Fe": 0.000020,
                      "Zn": 0.000020}),
    "soft-tissue-icrp": ("Tissue, Soft (ICRP)", 320, 307, 1.0, "NIST 1998 (compos.pl matno=261)",
                         {"H": 0.104472, "C": 0.232190, "N": 0.024880, "O": 0.630238, "Na": 0.001130, "Mg": 0.000130,
                          "P": 0.001330, "S": 0.001990, "Cl": 0.001340, "K": 0.001990, "Ca": 0.000230, "Fe": 0.000050,
                          "Zn": 0.000030}),
    "muscle-skeletal-icrp": ("Muscle, Skeletal (ICRP)", 201, 197, 1.04, "NIST 1998 (compos.pl matno=201)",
                             {"H": 0.100637, "C": 0.107830, "N": 0.027680, "O": 0.754773, "Na": 0.000750,
                              "Mg": 0.000190, "P": 0.001800, "S": 0.002410, "Cl": 0.000790, "K": 0.003020,
                              "Ca": 0.000030, "Fe": 0.000040, "Zn": 0.000050}),
    "brain-icrp": ("Brain (ICRP)", 44, 54, 1.03, "NIST 1998 (compos.pl matno=123)",
                   {"H": 0.110667, "C": 0.125420, "N": 0.013280, "O": 0.737723, "Na": 0.001840, "Mg": 0.000150,
                    "P": 0.003540, "S": 0.001770, "Cl": 0.002360, "K": 0.003100, "Ca": 0.000090, "Fe": 0.000050,
                    "Zn": 0.000010}),
    "skin-icrp": ("Skin (ICRP)", 286, 274, 1.10, "NIST 1998 (compos.pl matno=250)",
                  {"H": 0.100588, "C": 0.228250, "N": 0.046420, "O": 0.619002, "Na": 0.000070, "Mg": 0.000060,
                   "P": 0.000330, "S": 0.001590, "Cl": 0.002670, "K": 0.000850, "Ca": 0.000150, "Fe": 0.000010,
                   "Zn": 0.000010}),
    "blood-icrp": ("Blood (ICRP)", 29, 41, 1.06, "NIST 1998 (compos.pl matno=118)",
                   {"H": 0.101866, "C": 0.100020, "N": 0.029640, "O": 0.759414, "Na": 0.001850, "Mg": 0.000040,
                    "Si": 0.000030, "P": 0.000350, "S": 0.001850, "Cl": 0.002780, "K": 0.001630, "Ca": 0.000060,
                    "Fe": 0.000460}),
    "lung-icrp": ("Tissue, Lung (ICRP)", 318, 305, 1.05, "NIST 1998 (compos.pl matno=190)",
                  {"H": 0.101278, "C": 0.102310, "N": 0.028650, "O": 0.757072, "Na": 0.001840, "Mg": 0.000730,
                   "P": 0.000800, "S": 0.002250, "Cl": 0.002660, "K": 0.001940, "Ca": 0.000090, "Fe": 0.000370,
                   "Zn": 0.000010}),
    "cortical-bone-icrp": ("Bone, Cortical (ICRP)", 33, 45, 1.85, "NIST 1998 (compos.pl matno=120)",
                           {"H": 0.047234, "C": 0.144330, "N": 0.041990, "O": 0.446096, "Mg": 0.002200,
                            "P": 0.104970, "S": 0.003150, "Ca": 0.209930, "Zn": 0.000100}),
}
# Derived, declared-density variant: composition of "Tissue, Lung (ICRP)" at a
# declared inflated-lung density. The density is NOT read from PNNL (which
# lists 1.05 g/cm3 for lung tissue); it is a declared value.
M["lung-inflated-declared"] = (
    "Tissue, Lung (ICRP)", 318, 305, 0.26,
    "composition identical to lung-icrp; density DECLARED 0.26 g/cm3 (the inflated-lung value commonly "
    "attributed to ICRU 44; not verified against the ICRU 44 text in this work; not a PNNL-15870 value)",
    M["lung-icrp"][5])

# Generic head-CT demonstration anchors: (HU, material key)
ANCHORS = [
    (-1000.0, "air-dry"), (-740.0, "lung-inflated-declared"), (-100.0, "adipose-icrp"),
    (0.0, "water-liquid"), (30.0, "brain-icrp"), (50.0, "muscle-skeletal-icrp"),
    (60.0, "skin-icrp"), (1200.0, "cortical-bone-icrp"),
]


def expand(elem_wf):
    """Element weight fractions -> ({nuclide: mass fraction}, dropped, scale)."""
    raw = {}
    dropped = {}
    for el, w in elem_wf.items():
        if el in UNSUPPORTED:
            dropped[el] = dropped.get(el, 0.0) + w
            continue
        kept, lost = ISO[el]
        tot = sum(a * m for _, a, m in kept) + sum(a * m for a, m in lost)
        for name, a, m in kept:
            raw[name] = raw.get(name, 0.0) + w * a / tot * m
        if lost:
            dropped[el + "_unevaluated_isotopes"] = w * sum(a * m for a, m in lost) / tot
    scale = 1.0 / math.fsum(raw.values())
    out = {k: v * scale for k, v in raw.items()}
    big = max(out, key=out.get)
    out[big] += 1.0 - math.fsum(out.values())
    return out, dropped, scale


def sort_key(n):
    el = "".join(c for c in n if c.isalpha())
    return (ELEM_ORDER.index(el), int("".join(c for c in n if c.isdigit())))


def material(mid, spec):
    name, entry, page, rho, src, wf = spec
    nucs, dropped, scale = expand(wf)
    doc = {
        "schema_version": "openbnct.material-definition/0.1.0",
        "id": f"openbnct.tissue-library.{mid}.v1",
        "density_g_cm3": rho,
        "temperature_k": T_K,
        "nuclides": [{"name": n, "mass_fraction": nucs[n]} for n in sorted(nucs, key=sort_key)],
        "neutron_thermal_treatment": "free_gas",
    }
    audit = {
        "printed_name": name, "printed_entry": entry, "printed_page_of_357": page,
        "printed_element_weight_fractions": wf, "printed_sum": round(math.fsum(wf.values()), 6),
        "dropped_no_local_evaluation": dropped,
        "renormalization_factor": scale, "source_comment": src,
    }
    return doc, audit


def dump(path, obj):
    with open(path, "w") as f:
        json.dump(obj, f, indent=1)
        f.write("\n")


def main():
    os.makedirs(os.path.join(HERE, "materials"), exist_ok=True)
    audits, docs = {}, {}
    for mid, spec in M.items():
        docs[mid], audits[mid] = material(mid, spec)
        dump(os.path.join(HERE, "materials", f"{mid}.json"), docs[mid])
    cal = {
        "schema_version": "openbnct.hu-calibration/0.1.0",
        "id": "openbnct.tissue-library.hu-calibration-generic-head-ct.v1",
        "anchors": [{"hu": hu, "material": docs[k]} for hu, k in ANCHORS],
        "derivation": None,
        "validity_domain": (
            "GENERIC DEMONSTRATION TABLE -- NOT a scanner calibration. Declared convention: each anchor pairs a "
            "PNNL-15870 Rev. 1 tissue composition with a conventional head-CT Hounsfield position chosen by the "
            "library authors (air -1000, inflated lung -740, adipose -100, water 0, brain 30, skeletal muscle 50, "
            "skin 60, cortical bone 1200); a voxel between anchors becomes the two-component volume mixture "
            "(1-t)*A_i + t*A_(i+1), the stoichiometric-calibration structure of Schneider, Bortfeld & Schlegel, "
            "Phys. Med. Biol. 45 (2000) 459 -- whose fitted tables were NOT used or reproduced here. The anchor HU "
            "values are not derived from any scanner, kVp or reconstruction kernel, and were not read from that "
            "paper. Real deployments must substitute a site-derived anchor table. Blood, generic soft tissue and "
            "deflated lung are library materials but not anchors (not separable from the anchors by CT number)."),
        "provenance_id": "libraries/tissue",
    }
    dump(os.path.join(HERE, "hu-calibration-generic-head-ct.json"), cal)
    dump(os.path.join(HERE, "audit.json"), {"source": PNNL, "materials": audits})


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Write libraries/tissue/manifest.json (sha256 per file) from audit.json and the
files on disk. Run after generate.py and collapse-28g.sh."""
import hashlib
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))


def sha(rel):
    h = hashlib.sha256()
    with open(os.path.join(HERE, rel), "rb") as f:
        h.update(f.read())
    return h.hexdigest()


audit = json.load(open(os.path.join(HERE, "audit.json")))
materials = []
for mid, a in audit["materials"].items():
    rel = f"materials/{mid}.json"
    doc = json.load(open(os.path.join(HERE, rel)))
    materials.append({
        "id": doc["id"], "file": rel, "sha256": sha(rel),
        "density_g_cm3": doc["density_g_cm3"],
        "source": {
            "document": audit["source"],
            "printed_material_name": a["printed_name"],
            "printed_entry_number": a["printed_entry"],
            "printed_page_of_357": a["printed_page_of_357"],
            "underlying_tabulation": a["source_comment"],
            "printed_element_weight_fractions": a["printed_element_weight_fractions"],
        },
        "expansion": {
            "rule": "element -> locally evaluated natural nuclides, natural abundance converted to mass fraction; "
                    "elements/isotopes without a local evaluation dropped (never substituted) and the rest renormalized",
            "dropped_mass_fraction_before_renormalization": a["dropped_no_local_evaluation"],
            "renormalization_factor": a["renormalization_factor"],
        },
    })
ntb = {
    "schema_version": "openbnct.library-manifest/0.1.0",
    "id": "openbnct.tissue-library.v1",
    "note": ("Standard tissue library for head-CT-to-transport workflows: research inputs, not clinical "
             "commissioning data. Compositions are element weight fractions printed in PNNL-15870 Rev. 1 "
             "(Detwiler et al. 2021, Pacific Northwest National Laboratory), which reproduce NIST 1998 / ICRP "
             "reference-man tabulations; each entry below records the exact printed name, entry number and page. "
             "Omitted for want of a readable authoritative source: spongiosa / trabecular bone, red and yellow "
             "marrow, ICRU-44 inflated-lung density. Regenerate with generate.py, collapse-28g.sh, make-manifest.py."),
    "materials": materials,
    "hu_calibration": {
        "id": "openbnct.tissue-library.hu-calibration-generic-head-ct.v1",
        "file": "hu-calibration-generic-head-ct.json",
        "sha256": sha("hu-calibration-generic-head-ct.json"),
        "status": "generic demonstration table, not a scanner calibration",
        "anchors_hu": [-1000, -740, -100, 0, 30, 50, 60, 1200],
        "anchor_citation": ("Structure: Schneider, Bortfeld & Schlegel, Phys. Med. Biol. 45 (2000) 459-478. "
                            "The anchor HU positions are conventional values chosen by the library authors; they "
                            "were not read from, fitted to, or derived from that paper's tables."),
    },
    "multigroup_data": {
        "id": "openbnct.tissue-library.multigroup-28g-tsl.v1",
        "file": "multigroup-data-28g-tsl.json",
        "sha256": sha("multigroup-data-28g-tsl.json"),
        "note": ("Default. Hydrogen in every material carries the ENDF/B-VIII.1 H-in-H2O thermal-scattering "
                 "law (293.6 K), giving thermal upscatter. multigroup-data-28g.json (id "
                 "openbnct.tissue-library.multigroup-28g.v1, free-gas kernel) is retained for provenance only."),
        "free_gas_predecessor": {"id": "openbnct.tissue-library.multigroup-28g.v1",
                                 "file": "multigroup-data-28g.json",
                                 "sha256": sha("multigroup-data-28g.json")},
        "recipe": "collapse-28g.sh (recipe of benchmarks/synthetic/layered-head-phantom/collapse-v2.sh)",
        "nuclear_data": "ENDF/B-VIII.1: H/C/N/O/Fe56/B10 from 294 K OpenMC-HDF5; Na/Mg/P/S/Cl/K/Ca from NJOY 293.6 K PENDF; H-in-H2O S(alpha,beta) tsl_H(H2O)_0001 on hydrogen",
        "component_profile": "benchmarks/synthetic/nf-bnct-001/transport/component-profile-local-kerma.json",
    },
    "generators": {f: sha(f) for f in ("generate.py", "collapse-28g.sh", "make-manifest.py")},
}
with open(os.path.join(HERE, "manifest.json"), "w") as f:
    json.dump(ntb, f, indent=1)
    f.write("\n")

#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Worked OpenBNCT research workflow — the Python parity surface end to end.

Two halves, both runnable today against committed artifacts:

  1. Case pipeline — generate the synthetic NF-BNCT-001 case, verify its
     hash-bound manifest, inspect geometry/structures, aim the plane
     source, and emit the MCNP reproduction deck. The transport run
     itself is the external step (OpenMC or a licensed engine) and is not
     exercised here.
  2. Dose analysis — on the committed 2-voxel conformance bundle, compute
     a DVH and region metrics, apply the fixed-weights biological model,
     and compare a bundle against itself through the comparison record.

Run from the repository root with the wheel installed:

    pip install bindings/python/dist/openbnct-*.whl   # or maturin develop
    python examples/python/workflow.py

Research use only — nothing here is a clinical, equivalence, or
commissioning claim.
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

import openbnct

REPO = Path(__file__).resolve().parents[2]
BIO = REPO / "conformance" / "bio" / "0.2.0" / "cases"
TRANSPORT = (
    REPO / "benchmarks" / "synthetic" / "nf-bnct-001" / "transport"
)


def case_pipeline(work: Path) -> None:
    """Generate, verify, load, aim the source, export the MCNP deck."""
    case_dir = work / "nf-bnct-001"
    generated = openbnct.generate_case(case_dir)
    print(f"generated case at {generated.root} "
          f"({generated.ct_file_count} CT files)")

    report = openbnct.verify_case(case_dir)
    print(f"verify: {report.case_id} — "
          f"{report.verified_artifact_count} artifacts hash-verified")

    case = openbnct.load_case(case_dir)
    geometry = case.geometry
    print(f"geometry: {geometry.shape} voxels "
          f"@ {geometry.spacing_mm} mm spacing")
    print("structures:", [s.name for s in case.structures])

    mask = case.structure_mask("CORE")
    included = sum(mask)
    print(f"CORE structure mask: {included} voxels")

    # Source positioning is contract-driven — the mask rides as a
    # RegionMask document, then aim persists the moved source.
    mask_file = work / "core-mask.json"
    mask_file.write_text(json.dumps({"name": "CORE", "voxels": mask}))
    source = openbnct.load_fixed_source(TRANSPORT / "source.json")
    aimed, report = openbnct.aim_source(
        source, case.geometry, mask_file, case.report.case_id,
        approach="-z",
    )
    (work / "source-aimed.json").write_text(aimed.to_json())
    entry_axis, entry_side = report.entry
    print(f"aimed {aimed.id} at centroid {report.target_centroid_lps_mm}, "
          f"entry {entry_side} of {entry_axis}")

    deck = work / "nf-bnct-001.i"
    openbnct.export_mcnp_deck(
        TRANSPORT / "case.json", deck, xs_suffix="80c", seed=42
    )
    text = deck.read_text()
    assert "fmesh" in text and "sdef" in text, "deck missing tallies/source"
    print(f"MCNP deck: {len(text.splitlines())} lines")


def dose_analysis(work: Path) -> None:
    """DVH, region metrics, and biological weighting on conformance data."""
    bundle = openbnct.load_physical_dose_bundle(BIO / "physical-bundle.json")
    print(f"bundle: {bundle.case_id}, "
          f"{len(bundle.components)} components, "
          f"provenance {bundle.provenance_id[:40]}…")

    mask = json.loads((BIO / "mask-core.json").read_text())
    voxels = mask["voxels"]

    dvh = openbnct.compute_dvh(
        bundle, "physical_total", "core", voxels, bins=4
    )
    print(f"DVH[{dvh.quantity} @ {dvh.region}]: "
          f"{list(dvh.dose_edges)} {dvh.unit}")

    metrics = openbnct.compute_metrics(
        bundle, "physical_total", "core", voxels,
        dx=[98.0, 50.0, 2.0], vx=[2.0], eud=[8.0],
    )
    print(f"metrics: mean {metrics.mean_dose:.4g} {metrics.unit}, "
          f"D98 {metrics.dx[0][1]:.4g}")

    model = openbnct.load_biological_model(BIO / "model-fixed-weights.json")
    biological = openbnct.apply_model(
        model, bundle, [("core", BIO / "mask-core.json")]
    )
    out = work / "biological-bundle.json"
    biological.write(out)
    print(f"biological bundle written ({out.name}) — "
          f"research weights, not a clinical quantity")

    comparison = openbnct.compare_dose_bundles(bundle, bundle, sigma_level=2.0)
    record = json.loads(comparison.to_json())
    print(f"self-compare: {record['voxel_count']} voxels, "
          f"schema {record['schema_version']}")


def main() -> int:
    print("== OpenBNCT worked workflow ==")
    with tempfile.TemporaryDirectory(prefix="openbnct-example-") as tmp:
        work = Path(tmp)
        print("-- case pipeline")
        case_pipeline(work)
        print("-- dose analysis")
        dose_analysis(work)
    print("done — outputs lived under a temp dir and are gone")
    return 0


if __name__ == "__main__":
    sys.exit(main())

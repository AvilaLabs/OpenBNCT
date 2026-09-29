#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Worked OpenBNCT research workflow — the Python parity surface end to end.

Three parts, all runnable today against committed artifacts:

  1. Case pipeline — generate the synthetic NF-BNCT-001 case, verify its
     hash-bound manifest, inspect geometry/structures, aim the plane
     source, and emit the MCNP reproduction deck. The transport run
     itself is the external step (OpenMC or a licensed engine) and is not
     exercised here.
  2. Dose analysis — on the committed 2-voxel conformance bundle, compute
     a DVH and region metrics, apply the fixed-weights biological model,
     and compare a bundle against itself through the comparison record.
  3. Solve and NumPy — run the deterministic S_N solver from Python
     (`openbnct.sn_solve`, the same Rust library path as `openbnct sn
     solve`), take the flux and dose as NumPy arrays, compute a region
     mean, and apply a 10B concentration with `boron_dose`. Pass
     `--layered-head` to also solve the layered head phantom (seconds in
     a release build, minutes in a debug wheel, so off by default).

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
import warnings
from pathlib import Path

import numpy as np

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


NF003 = REPO / "benchmarks" / "synthetic" / "nf-bnct-003" / "transport"
HEAD = REPO / "benchmarks" / "synthetic" / "layered-head-phantom"


def solve_and_numpy(work: Path) -> None:
    """S_N solve -> NumPy arrays -> region mean -> boron dose."""
    # The nf-bnct-003 verification fixture: a 4 x 4 x 40 voxel, one-group
    # ideal 10B absorber slab (tiny, so this runs in well under a second
    # even in a debug wheel). Its data has no collapsed 10B unit response,
    # so this MECHANICS DEMO attaches an illustrative one (the fixture's
    # boron response taken per 1e6 ug/g). The resulting boron doses show
    # the API; they are not physical predictions.
    data = json.loads((NF003 / "multigroup-data.json").read_text())
    fixture_response = data["materials"][0]["dose_response_gy_cm2"]["boron"]
    data["boron_unit_response_gy_cm2_per_ug_g"] = [
        value / 1.0e6 for value in fixture_response
    ]
    data_file = work / "multigroup-data-unit.json"
    data_file.write_text(json.dumps(data))

    solution = openbnct.sn_solve(
        NF003 / "case.json", data_file, dose=True, boron_unit=True
    )
    flux, dose = solution.flux, solution.dose
    print(f"solve: {solution!r}")

    # Axis order: every voxel array is C-order (nz, ny, nx); flux adds a
    # leading group axis. array[k, j, i] = column i, row j, slice k.
    phi = flux.as_array()
    total = dose.physical_total.as_array()
    nz, ny, nx = total.shape
    print(f"flux {phi.shape} (groups, nz, ny, nx); dose {total.shape} "
          f"(nz, ny, nx); spacing {dose.physical_total.spacing_mm} mm")
    assert phi.shape == (flux.group_count, nz, ny, nx)
    assert np.isfinite(phi).all() and np.isfinite(total).all()

    # A region is just a NumPy boolean array; ravel() (C order) is the
    # x-fastest flat order that the mask files and lists use.
    front = np.zeros((nz, ny, nx), dtype=bool)
    front[: nz // 2] = True
    print(f"front-half mean flux {phi[0][front].mean():.4g} vs "
          f"back-half {phi[0][~front].mean():.4g} (cm^-2 s^-1 per source)")
    mask_file = work / "mask-front.json"
    mask_file.write_text(
        json.dumps({"name": "front", "voxels": front.ravel().tolist()})
    )

    # 10B concentration: 30 ug/g in blood, tissue:blood ratio 3.5 in the
    # front region, 1.0 elsewhere (trace-10B approximation).
    with_boron = openbnct.boron_dose(
        dose,
        solution.boron_unit_dose,
        blood_ug_g=30.0,
        ratios={"front": 3.5},
        masks=[("front", mask_file)],
        default_ratio=1.0,
    )
    boron = with_boron.physical_total.as_array()
    # The fixture's own dose is a 1e6 ug/g (pure 10B) absorber, so the
    # 30 * 3.5 = 105 ug/g field scales the boron dose by 105 / 1e6.
    print(f"front-half mean dose: {total[front].mean():.4g} (fixture) -> "
          f"{boron[front].mean():.4g} Gy per source particle at 105 ug/g")
    assert np.isfinite(boron).all()
    assert np.isclose(boron[front].mean() / total[front].mean(), 105.0e-6)


def layered_head() -> None:
    """Optional: the layered head phantom (15625 voxels, 28 groups)."""
    case = openbnct.load_transport_case(HEAD / "case.json")
    print(f"layered head: {case.geometry.array_shape} voxels, S4")
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        solution = openbnct.sn_solve(
            case,
            HEAD / "multigroup-data-28g-v3-shielded.json",
            HEAD / "assignment.json",
            order=4,
            max_outer=8,
            allow_unconverged=True,  # labeled: the field may be provisional
        )
    total = solution.dose.physical_total.as_array()
    print(f"converged={solution.converged}; dose {total.shape}, "
          f"peak {total.max():.4g} Gy per source particle")


def main() -> int:
    print("== OpenBNCT worked workflow ==")
    with tempfile.TemporaryDirectory(prefix="openbnct-example-") as tmp:
        work = Path(tmp)
        print("-- case pipeline")
        case_pipeline(work)
        print("-- dose analysis")
        dose_analysis(work)
        print("-- solve and NumPy")
        solve_and_numpy(work)
    if "--layered-head" in sys.argv[1:]:
        print("-- layered head phantom (slow in a debug build)")
        layered_head()
    print("done — outputs lived under a temp dir and are gone")
    return 0


if __name__ == "__main__":
    sys.exit(main())

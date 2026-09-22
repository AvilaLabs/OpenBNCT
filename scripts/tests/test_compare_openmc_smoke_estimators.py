# SPDX-License-Identifier: MIT

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import h5py
import numpy


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
COMPARATOR = REPOSITORY_ROOT / "scripts" / "compare-openmc-smoke-estimators.py"
N_REALIZATIONS = 5
VOXELS = 2
EV_PER_JOULE = 1.0 / 1.602176634e-19


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sha256_text(document: dict) -> str:
    return hashlib.sha256(
        (json.dumps(document, indent=2) + "\n").encode("utf-8")
    ).hexdigest()


def endf_number(value: float) -> str:
    mantissa, exponent = f"{value:.6E}".split("E")
    return f"{mantissa}{int(exponent):+d}".rjust(11)


def endf_integer(value: int) -> str:
    return f"{value:11d}"


def endf_record(fields: list[str], mat: int, mf: int, mt: int, sequence: int) -> str:
    return "".join(fields) + f"{mat:4d}{mf:2d}{mt:3d}{sequence:5d}\n"


def endf_tab1_records(mat: int, mt: int, energies: list[float], values: list[float]) -> list[str]:
    records = [
        endf_record(
            [endf_number(1001.0), endf_number(1.0)] + [endf_integer(0)] * 4,
            mat,
            3,
            mt,
            1,
        ),
        endf_record(
            [endf_number(0.0), endf_number(0.0)]
            + [endf_integer(0), endf_integer(0), endf_integer(1), endf_integer(len(energies))],
            mat,
            3,
            mt,
            2,
        ),
        endf_record(
            [endf_integer(len(energies)), endf_integer(2)] + [endf_integer(0)] * 4,
            mat,
            3,
            mt,
            3,
        ),
    ]
    pairs = []
    for energy, value in zip(energies, values, strict=True):
        pairs.extend((endf_number(energy), endf_number(value)))
    while pairs:
        fields, pairs = pairs[:6], pairs[6:]
        fields += [endf_integer(0)] * (6 - len(fields))
        records.append(endf_record(fields, mat, 3, mt, len(records) + 1))
    return records


def write_pendf(path: Path, mat: int, sections: dict[int, tuple[list[float], list[float]]]) -> None:
    records = []
    for mt, (energies, values) in sections.items():
        records.extend(endf_tab1_records(mat, mt, energies, values))
        records.append(endf_record([endf_integer(0)] * 6, mat, 3, 0, len(records) + 1))
    records.append(endf_record([endf_integer(0)] * 6, mat, 0, 0, len(records) + 1))
    path.write_text("".join(records), encoding="ascii", newline="\n")


def moments(mean: float, std: float) -> tuple[float, float]:
    summed = mean * N_REALIZATIONS
    summed_sq = N_REALIZATIONS * (
        mean * mean + (N_REALIZATIONS - 1) * std * std
    )
    return summed, summed_sq


def tally_results(bins: int, mean: float, std: float) -> numpy.ndarray:
    summed, summed_sq = moments(mean, std)
    results = numpy.zeros((bins, 1, 2), dtype=float)
    results[:, 0, 0] = summed
    results[:, 0, 1] = summed_sq
    return results


# Means are chosen so response/rate reproduces each PENDF's KERMA/xs ratio:
# boron 2.0e6 eV per reaction, nitrogen 6.26e5 eV per reaction at density 1.
TALLY_DEFS = [
    # (tally_id, name, filter_ids, bins, mean, std)
    (1, "openbnct.component.boron.response", [1, 2, 4], VOXELS, 1.3684e-12, 1.0e-14),
    (2, "openbnct.component.nitrogen.response", [1, 2, 5], VOXELS, 1.6018e-13, 2.0e-15),
    (3, "openbnct.component.hydrogen.response", [1, 2, 6], VOXELS, 5.0e-13, 5.0e-15),
    (4, "openbnct.audit.neutron_heating", [1, 2], VOXELS, 170000.0, 1000.0),
    (5, "openbnct.component.photon.heating", [1, 3], VOXELS, 20000.0, 400.0),
    (6, "openbnct.physical_total.coupled_heating", [1], VOXELS, 190000.0, 1100.0),
    (7, "openbnct.audit.b10_mt107", [1, 2], VOXELS, 0.0042692, 4.0e-5),
    (8, "openbnct.audit.n14_mt103", [1, 2], VOXELS, 0.0015976, 1.6e-5),
    (9, "openbnct.diagnostic.neutron_fluence", [1, 2, 7], VOXELS * 4, 0.01, 1.0e-4),
    (10, "openbnct.diagnostic.photon_fluence", [1, 3, 8], VOXELS * 5, 0.002, 2.0e-5),
    (11, "openbnct.diagnostic.neutron_surface_current", [9, 2], 6, -0.02, 2.0e-4),
    (12, "openbnct.diagnostic.photon_surface_current", [9, 3], 6, -0.003, 3.0e-5),
]

RESPONSE_ENERGY = [1.0e-5, 1.0, 2.0e7]
RESPONSE_CURVES = {
    "boron": [2.0e-10, 1.0e-10, 5.0e-11],
    "nitrogen": [4.0e-11, 2.0e-11, 1.0e-11],
    "hydrogen": [1.0e-10, 5.0e-11, 2.5e-11],
}


def write_statepoint(path: Path) -> None:
    with h5py.File(path, "w") as handle:
        handle.create_dataset("current_batch", data=N_REALIZATIONS)
        handle.create_dataset("energy_mode", data=numpy.bytes_("continuous-energy"))
        handle.create_dataset("n_batches", data=N_REALIZATIONS)
        handle.create_dataset("n_particles", data=200)
        handle.create_dataset("n_realizations", data=N_REALIZATIONS)
        handle.create_dataset("run_mode", data=numpy.bytes_("fixed source"))
        handle.create_dataset("seed", data=20260831)
        handle.create_dataset("stride", data=152917)
        tallies = handle.create_group("tallies")
        filters = tallies.create_group("filters")
        for filter_id, (kind, payload) in {
            1: ("mesh", None),
            2: ("particle", [1]),
            3: ("particle", [2]),
            4: ("energyfunction", RESPONSE_ENERGY),
            5: ("energyfunction", RESPONSE_ENERGY),
            6: ("energyfunction", RESPONSE_ENERGY),
            7: ("energy", [0.0, 0.5, 1000.0, 10000.0, 2.0e7]),
            8: ("energy", [0.0, 477000.0, 479000.0, 2223000.0, 2225000.0, 2.0e7]),
            9: ("surface", [1, 2, 3, 4, 5, 6]),
        }.items():
            group = filters.create_group(f"filter {filter_id}")
            group.create_dataset("type", data=numpy.bytes_(kind))
            if kind == "energyfunction":
                group.create_dataset("energy", data=payload)
                component = {4: "boron", 5: "nitrogen", 6: "hydrogen"}[filter_id]
                group.create_dataset("y", data=RESPONSE_CURVES[component])
                group.create_dataset("n_bins", data=len(payload))
            else:
                bins = payload if payload is not None else [0, VOXELS]
                group.create_dataset("bins", data=bins)
                group.create_dataset("n_bins", data=len(bins) - 1)
        for tally_id, name, filter_ids, bins, mean, std in TALLY_DEFS:
            group = tallies.create_group(f"tally {tally_id}")
            group.create_dataset("name", data=numpy.bytes_(name))
            group.create_dataset("estimator", data=numpy.bytes_("tracklength"))
            group.create_dataset("filters", data=filter_ids)
            group.create_dataset("n_filters", data=len(filter_ids))
            group.create_dataset("n_score_bins", data=1)
            group.create_dataset("n_realizations", data=N_REALIZATIONS)
            group.create_dataset("results", data=tally_results(bins, mean, std))


class ComparatorTest(unittest.TestCase):
    def test_writes_bound_report_and_rejects_tampering(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            execution_root = root / "execution"
            deck_root = root / "deck"
            for nuclide in ("B10", "N14"):
                (execution_root / nuclide).mkdir(parents=True)
            deck_root.mkdir()

            energies = [1.0e-5, 0.1, 1.0, 1.0e3, 1.0e4, 2.0e7]
            b10_path = execution_root / "B10" / "tape23"
            # E_dep = KERMA/xs = 2.0e6 eV per reaction everywhere.
            write_pendf(
                b10_path,
                525,
                {
                    107: (energies, [10.0, 5.0, 2.0, 1.0, 0.5, 0.1]),
                    407: (energies, [2.0e7, 1.0e7, 4.0e6, 2.0e6, 1.0e6, 2.0e5]),
                },
            )
            n14_path = execution_root / "N14" / "tape23"
            write_pendf(
                n14_path,
                725,
                {
                    103: (energies, [1.0, 0.5, 0.2, 0.1, 0.05, 0.01]),
                    403: (energies, [6.26e5, 3.13e5, 1.252e5, 6.26e4, 3.13e4, 6.26e3]),
                },
            )

            receipt = {
                "schema_version": "openbnct.njoy-execution-receipt/0.1.0",
                "id": "synthetic-njoy-execution",
                "case_id": "nf-bnct-001",
                "processor": {"tool": {"version": "2016.78"}},
                "runs": [
                    {
                        "nuclide": nuclide,
                        "exit_code": 0,
                        "production_diagnostic_pendf_identical": True,
                        "output_tapes": [
                            {
                                "unit": 23,
                                "purpose": "production_heatr_pendf",
                                "artifact": {
                                    "path": f"{nuclide}/tape23",
                                    "media_type": "application/x-endf",
                                    "size_bytes": (execution_root / nuclide / "tape23").stat().st_size,
                                    "sha256": sha256(execution_root / nuclide / "tape23"),
                                },
                            }
                        ],
                    }
                    for nuclide, path in (("B10", b10_path), ("N14", n14_path))
                ],
            }
            receipt_path = execution_root / "openbnct-njoy-execution-receipt.json"
            receipt_path.write_text(
                json.dumps(receipt, indent=2) + "\n", encoding="utf-8", newline="\n"
            )

            response_set = {
                "schema_version": "openbnct.neutron-response-set/0.1.0",
                "id": "synthetic-response-set",
                "qualification": "independently_reviewed",
                "energy_ev": RESPONSE_ENERGY,
                "unit": "gray_square_centimeter",
                "interpolation": "linear_linear",
                "boron_gy_cm2": RESPONSE_CURVES["boron"],
                "nitrogen_gy_cm2": RESPONSE_CURVES["nitrogen"],
                "hydrogen_gy_cm2": RESPONSE_CURVES["hydrogen"],
                "total_neutron_gy_cm2": [
                    b + n + h
                    for b, n, h in zip(
                        RESPONSE_CURVES["boron"],
                        RESPONSE_CURVES["nitrogen"],
                        RESPONSE_CURVES["hydrogen"],
                        strict=True,
                    )
                ],
            }
            material = {
                "schema_version": "openbnct.material-definition/0.1.0",
                "id": "synthetic-material",
                "density_g_cm3": 1.0,
            }
            profile = {
                "schema_version": "openbnct.openmc-execution-profile/0.1.0",
                "id": "synthetic-smoke-profile",
                "purpose": "smoke_only",
                "openmc_version": "0.16.0",
                "batches": N_REALIZATIONS,
                "seed": 20260831,
                "energy_mode": "continuous-energy",
            }
            documents = {
                "response_set": response_set,
                "material": material,
                "execution_profile": profile,
            }
            manifest = {
                "schema_version": "openbnct.openmc-input-manifest/0.1.0",
                "case_id": "nf-bnct-001",
                "backend_id": "openmc",
                "openmc_version": "0.16.0",
                "openmc_source_commit": "617d35a5063c57796b43428bc401e627d2011046",
                "bindings": {
                    key: {
                        "id": document["id"],
                        "sha256": sha256_text(document),
                    }
                    for key, document in documents.items()
                },
                "execution": {
                    "purpose": "smoke_only",
                    "requested_histories": 1000,
                    "batches": N_REALIZATIONS,
                    "particles_per_batch": 200,
                    "seed": 20260831,
                    "stride": 152917,
                },
                "scoring_mesh": {
                    "mesh_id": 1,
                    "dimensions": [2, 1, 1],
                    "lower_left_cm": [-1.0, -1.0, -1.0],
                    "upper_right_cm": [1.0, 1.0, 1.0],
                    "voxel_volume_cm3": 4.0,
                    "voxel_mass_g": 4.0,
                    "cell_volume_cm3": 8.0,
                    "cell_mass_g": 8.0,
                },
                "tallies": [
                    {
                        "id": tally_id,
                        "name": name,
                        "component": None,
                        "particle": None,
                        "quantity": "response_weighted_track_length",
                        "raw_unit": "gray_cubic_centimeter_per_source_neutron",
                        "collection_normalization": "divide_by_voxel_volume_cm3",
                    }
                    for tally_id, name, _, _, _, _ in TALLY_DEFS
                ],
            }
            manifest_path = deck_root / "openbnct-input-manifest.json"
            manifest_path.write_text(
                json.dumps(manifest, indent=2) + "\n", encoding="utf-8", newline="\n"
            )
            statepoint_path = deck_root / "statepoint.5.h5"
            write_statepoint(statepoint_path)

            paths = {}
            for key, document in documents.items():
                path = root / f"{key}.json"
                path.write_text(
                    json.dumps(document, indent=2) + "\n",
                    encoding="utf-8",
                    newline="\n",
                )
                paths[key] = path

            output = root / "report.json"
            environment = os.environ.copy()
            environment["PYTHONPATH"] = os.pathsep.join(sys.path)
            command = [
                sys.executable,
                str(COMPARATOR),
                "--statepoint",
                str(statepoint_path),
                "--input-manifest",
                str(manifest_path),
                "--response-set",
                str(paths["response_set"]),
                "--material",
                str(paths["material"]),
                "--execution-profile",
                str(paths["execution_profile"]),
                "--execution-root",
                str(execution_root),
                "--execution-receipt",
                str(receipt_path),
                "--report-id",
                "synthetic-smoke-comparison",
                "--output",
                str(output),
            ]

            subprocess.run(command, check=True, env=environment)
            report = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(
                report["schema_version"],
                "openbnct.openmc-smoke-estimator-comparison/0.1.0",
            )
            self.assertTrue(
                report["tally_contract"]["all_contract_tallies_present"]
            )
            self.assertEqual(report["tally_contract"]["statepoint_tally_count"], 12)
            for check in report["executed_response_table_checks"]:
                self.assertTrue(
                    check["executed_tables_bitwise_identical_to_response_set"]
                )
            comparisons = report["estimator_comparisons"]
            closure = comparisons["coupled_heating_closure"]
            self.assertAlmostEqual(
                closure["coupled_ev_per_source_neutron"],
                2 * 190000.0,
            )
            # Component sums (Gy cm^3) are independent of the heating tallies in
            # this synthetic fixture, so only the audit identity is checked.
            self.assertLess(closure["difference_in_combined_sigma"], 3.0)
            audits = comparisons["reaction_rate_times_mean_deposited_energy"]
            for component, expected_mt in (("boron", 107), ("nitrogen", 103)):
                audit = audits[component]
                self.assertEqual(audit["reaction_mt"], expected_mt)
                self.assertAlmostEqual(
                    audit["measured_over_evaluated"], 1.0, delta=0.05
                )
            self.assertEqual(
                report["bindings"]["statepoint"]["sha256"], sha256(statepoint_path)
            )
            self.assertEqual(
                report["qualification"],
                "smoke_execution_evidence_not_dose_qualification",
            )

            with b10_path.open("ab") as stream:
                stream.write(b"tampered\n")
            failed = subprocess.run(
                [*command[:-1], str(root / "tampered-report.json")],
                check=False,
                capture_output=True,
                text=True,
                env=environment,
            )
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn("artifact size mismatch", failed.stderr)


if __name__ == "__main__":
    unittest.main()

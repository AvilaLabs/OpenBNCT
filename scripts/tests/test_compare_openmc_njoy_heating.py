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


K_BOLTZMANN_EV_PER_K = 8.617333262e-5
REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
COMPARATOR = REPOSITORY_ROOT / "scripts" / "compare-openmc-njoy-heating.py"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def endf_number(value: float) -> str:
    mantissa, exponent = f"{value:.6E}".split("E")
    return f"{mantissa}{int(exponent):+d}".rjust(11)


def endf_integer(value: int) -> str:
    return f"{value:11d}"


def endf_record(fields: list[str], mat: int, mf: int, mt: int, sequence: int) -> str:
    return "".join(fields) + f"{mat:4d}{mf:2d}{mt:3d}{sequence:5d}\n"


def write_pendf(path: Path, energies: list[float], values: list[float]) -> None:
    records = [
        endf_record(
            [endf_number(1001.0), endf_number(1.0)] + [endf_integer(0)] * 4,
            125,
            3,
            301,
            1,
        ),
        endf_record(
            [endf_number(0.0), endf_number(0.0)]
            + [endf_integer(0), endf_integer(0), endf_integer(1), endf_integer(len(energies))],
            125,
            3,
            301,
            2,
        ),
        endf_record(
            [endf_integer(len(energies)), endf_integer(2)] + [endf_integer(0)] * 4,
            125,
            3,
            301,
            3,
        ),
    ]
    pairs = []
    for energy, value in zip(energies, values, strict=True):
        pairs.extend((endf_number(energy), endf_number(value)))
    while pairs:
        fields, pairs = pairs[:6], pairs[6:]
        fields += [endf_integer(0)] * (6 - len(fields))
        records.append(endf_record(fields, 125, 3, 301, len(records) + 1))
    records.append(
        endf_record([endf_integer(0)] * 6, 125, 3, 0, len(records) + 1)
    )
    path.write_text("".join(records), encoding="ascii", newline="\n")


def write_hdf5(path: Path, energies: list[float], values: list[float]) -> None:
    with h5py.File(path, "w") as handle:
        handle.attrs["filetype"] = numpy.bytes_("data_neutron")
        nuclide = handle.create_group("H1")
        temperatures = nuclide.create_group("kTs")
        temperatures.create_dataset(
            "294K", data=293.6 * K_BOLTZMANN_EV_PER_K
        )
        energy = nuclide.create_group("energy")
        energy.create_dataset("294K", data=energies)
        reactions = nuclide.create_group("reactions")
        for mt in (301, 901):
            reaction = reactions.create_group(f"reaction_{mt:03}")
            reaction.attrs["mt"] = mt
            temperature = reaction.create_group("294K")
            dataset = temperature.create_dataset("xs", data=values)
            dataset.attrs["threshold_idx"] = 0


class ComparatorTest(unittest.TestCase):
    def test_writes_bound_pointwise_report_and_rejects_tampering(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data_root = root / "data"
            neutron_root = data_root / "neutron"
            execution_root = root / "execution"
            run_root = execution_root / "H1"
            neutron_root.mkdir(parents=True)
            run_root.mkdir(parents=True)
            energies = [1.0e-5, 1.0, 2.0e7]
            values = [1.0, 2.0, 3.0]

            hdf5_path = neutron_root / "H1.h5"
            write_hdf5(hdf5_path, energies, values)
            manifest = {
                "schema_version": "openbnct.openmc-nuclear-data-manifest/0.3.0",
                "id": "synthetic-openmc-manifest",
                "openmc_version": "0.16.0",
                "neutron_tables": [
                    {
                        "nuclide": "H1",
                        "artifact": {
                            "relative_path": "neutron/H1.h5",
                            "sha256": sha256(hdf5_path),
                        },
                        "photon_production_mts": [],
                    }
                ],
            }
            manifest_path = root / "manifest.json"
            manifest_path.write_text(
                json.dumps(manifest, indent=2) + "\n", encoding="utf-8", newline="\n"
            )

            pendf_path = run_root / "tape23"
            write_pendf(pendf_path, energies, values)
            receipt = {
                "schema_version": "openbnct.njoy-execution-receipt/0.1.0",
                "id": "synthetic-njoy-execution",
                "case_id": "nf-bnct-001",
                "processor": {"tool": {"version": "2016.78"}},
                "runs": [
                    {
                        "nuclide": "H1",
                        "exit_code": 0,
                        "production_diagnostic_pendf_identical": True,
                        "output_tapes": [
                            {
                                "unit": 23,
                                "purpose": "production_heatr_pendf",
                                "artifact": {
                                    "path": "H1/tape23",
                                    "media_type": "application/x-endf",
                                    "size_bytes": pendf_path.stat().st_size,
                                    "sha256": sha256(pendf_path),
                                },
                            }
                        ],
                    }
                ],
            }
            receipt_path = execution_root / "openbnct-njoy-execution-receipt.json"
            receipt_path.write_text(
                json.dumps(receipt, indent=2) + "\n", encoding="utf-8", newline="\n"
            )
            output = root / "report.json"
            environment = os.environ.copy()
            environment["PYTHONPATH"] = os.pathsep.join(sys.path)
            command = [
                sys.executable,
                str(COMPARATOR),
                "--data-root",
                str(data_root),
                "--manifest",
                str(manifest_path),
                "--execution-root",
                str(execution_root),
                "--execution-receipt",
                str(receipt_path),
                "--report-id",
                "synthetic-comparison",
                "--output",
                str(output),
            ]

            subprocess.run(command, check=True, env=environment)
            report = json.loads(output.read_text(encoding="utf-8"))
            self.assertTrue(report["summary"]["all_energy_grids_correspond"])
            self.assertTrue(report["summary"]["all_mt301_within_relative_tolerance"])
            self.assertEqual(
                report["summary"]["effective_local_photon_fallback_nuclides"],
                ["H1"],
            )
            self.assertEqual(report["results"][0]["maximum_relative_difference"], 0.0)
            grid_comparison = report["results"][0]["energy_grid_correspondence"]
            self.assertIn("maximum_absolute_difference_ev", grid_comparison)
            self.assertNotIn("maximum_absolute_difference_ev_barn", grid_comparison)
            self.assertEqual(
                report["bindings"]["openmc_nuclear_data_manifest"]["sha256"],
                sha256(manifest_path),
            )

            with pendf_path.open("ab") as stream:
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

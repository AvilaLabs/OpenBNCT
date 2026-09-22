# SPDX-License-Identifier: MIT

import hashlib
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest

import h5py
import numpy


K_BOLTZMANN_EV_PER_K = 8.617333262e-5
REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
INSPECTOR = REPOSITORY_ROOT / "scripts" / "inspect-openmc-data.py"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_acquisition_evidence(root: Path, archive: Path) -> tuple[Path, Path]:
    source_uri = "https://example.invalid/data.tar.xz"
    profile = {
        "schema_version": "openbnct.data-acquisition-profile/0.2.0",
        "id": "synthetic-openmc-data-profile",
        "artifact_role": "openmc_continuous_energy_library",
        "publication": {
            "publisher": "synthetic test publisher",
            "release_page_uri": "https://example.invalid/data/",
            "source_uri": source_uri,
            "allowed_https_host_suffixes": ["example.invalid"],
        },
        "artifact": {
            "filename": archive.name,
            "media_type": "application/x-xz",
            "expected_size_bytes": archive.stat().st_size,
            "expected_content_disposition_filename": archive.name,
            "publisher_digest": None,
            "known_prior_digests": [],
        },
        "size_evidence": {
            "method": "synthetic_test_fixture",
            "observed_on": "2026-08-31",
        },
        "upstream_recipe": None,
    }
    profile_path = root / "acquisition-profile.json"
    profile_path.write_text(
        json.dumps(profile, indent=2) + "\n", encoding="utf-8", newline="\n"
    )
    receipt = {
        "schema_version": "openbnct.data-acquisition-receipt/0.1.0",
        "profile_id": profile["id"],
        "profile_sha256": sha256(profile_path),
        "artifact_role": profile["artifact_role"],
        "artifact": {
            "path": archive.name,
            "media_type": profile["artifact"]["media_type"],
            "size_bytes": archive.stat().st_size,
            "sha256": sha256(archive),
            "publisher_digest": None,
        },
        "transfer": {
            "requested_uri": source_uri,
            "final_origin": "https://example.invalid",
            "resumed_from_bytes": 0,
            "content_disposition_filename": archive.name,
            "etag": None,
            "last_modified": None,
        },
        "publisher_digest_status": "unavailable",
        "evidence_state": "acquisition_only",
        "completed_at_unix_seconds": 1,
    }
    receipt_path = root / "acquisition-receipt.json"
    receipt_path.write_text(
        json.dumps(receipt, indent=2) + "\n", encoding="utf-8", newline="\n"
    )
    return profile_path, receipt_path


def write_neutron(path: Path, nuclide: str, reactions: dict[int, bool]) -> None:
    with h5py.File(path, "w") as handle:
        handle.attrs["filetype"] = numpy.bytes_("data_neutron")
        handle.attrs["version"] = numpy.array([3, 0])
        nuclide_group = handle.create_group(nuclide)
        nuclide_group.attrs["atomic_weight_ratio"] = 10.0
        temperatures = nuclide_group.create_group("kTs")
        temperatures.create_dataset(
            "294K", data=293.6 * K_BOLTZMANN_EV_PER_K
        )
        energies = nuclide_group.create_group("energy")
        energies.create_dataset("294K", data=[1.0e-5, 1.0e3, 20.0e6])
        reaction_group = nuclide_group.create_group("reactions")
        for mt, emits_photon in reactions.items():
            reaction = reaction_group.create_group(f"reaction_{mt:03}")
            reaction.attrs["mt"] = mt
            if emits_photon:
                product = reaction.create_group("product_0")
                product.attrs["particle"] = numpy.bytes_("photon")


def write_photon(path: Path, element: str) -> None:
    with h5py.File(path, "w") as handle:
        handle.attrs["filetype"] = numpy.bytes_("data_photon")
        handle.attrs["version"] = numpy.array([3, 0])
        element_group = handle.create_group(element)
        for name in ("coherent", "incoherent", "photoelectric"):
            element_group.create_group(name)
        subshells = element_group.create_group("subshells")
        subshells.attrs["designators"] = numpy.array([b"K"])
        shell = subshells.create_group("K")
        shell.attrs["binding_energy"] = 1.0
        shell.attrs["num_electrons"] = 1.0
        compton = element_group.create_group("compton_profiles")
        compton.create_dataset("num_electrons", data=[1.0])
        compton.create_dataset("binding_energy", data=[1.0])
        compton.create_dataset("pz", data=[0.0, 1.0])
        compton.create_dataset("J", data=[[1.0, 0.5]])


class InspectorTest(unittest.TestCase):
    def test_rejects_receipt_that_does_not_bind_archive(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "data.tar.xz"
            archive.write_bytes(b"synthetic archive")
            profile, receipt = write_acquisition_evidence(root, archive)
            receipt_document = json.loads(receipt.read_text(encoding="utf-8"))
            receipt_document["artifact"]["sha256"] = "0" * 64
            receipt.write_text(
                json.dumps(receipt_document, indent=2) + "\n",
                encoding="utf-8",
                newline="\n",
            )
            validate_acquisition = runpy.run_path(str(INSPECTOR))[
                "validate_acquisition"
            ]

            with self.assertRaisesRegex(ValueError, "archive SHA-256"):
                validate_acquisition(profile, receipt, archive)

    def test_extracts_case_scoped_capabilities_and_hashes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data_root = root / "data"
            neutron_root = data_root / "neutron"
            photon_root = data_root / "photon"
            neutron_root.mkdir(parents=True)
            photon_root.mkdir()

            neutron_capabilities = {
                "B10": {107: True, 301: False},
                "H1": {102: True, 301: False},
                "N14": {103: False, 301: False},
            }
            for nuclide, reactions in neutron_capabilities.items():
                write_neutron(neutron_root / f"{nuclide}.h5", nuclide, reactions)
            for element in ("B", "H", "N"):
                write_photon(photon_root / f"{element}.h5", element)

            libraries = []
            for nuclide in neutron_capabilities:
                libraries.append(
                    f'<library materials="{nuclide}" '
                    f'path="neutron/{nuclide}.h5" type="neutron"/>'
                )
            for element in ("B", "H", "N"):
                libraries.append(
                    f'<library materials="{element}" '
                    f'path="photon/{element}.h5" type="photon"/>'
                )
            libraries.append(
                '<library materials="H3" path="neutron/H3.h5" type="neutron"/>'
            )
            cross_sections = data_root / "cross_sections.xml"
            cross_sections.write_text(
                "<cross_sections>\n  "
                + "\n  ".join(libraries)
                + "\n</cross_sections>\n",
                encoding="utf-8",
            )

            material = root / "material.json"
            material.write_text(
                json.dumps(
                    {
                        "nuclides": [
                            {"name": "B10"},
                            {"name": "H1"},
                            {"name": "N14"},
                        ]
                    }
                ),
                encoding="utf-8",
            )
            archive = root / "data.tar.xz"
            archive.write_bytes(b"synthetic archive")
            acquisition_profile, acquisition_receipt = write_acquisition_evidence(
                root, archive
            )
            output = root / "manifest.json"
            environment = os.environ.copy()
            environment["PYTHONPATH"] = os.pathsep.join(sys.path)

            subprocess.run(
                [
                    sys.executable,
                    str(INSPECTOR),
                    "--data-root",
                    str(data_root),
                    "--cross-sections",
                    str(cross_sections),
                    "--material",
                    str(material),
                    "--archive",
                    str(archive),
                    "--acquisition-profile",
                    str(acquisition_profile),
                    "--acquisition-receipt",
                    str(acquisition_receipt),
                    "--distribution-id",
                    "synthetic-distribution",
                    "--manifest-id",
                    "synthetic-manifest",
                    "--output",
                    str(output),
                ],
                check=True,
                env=environment,
            )

            manifest = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(manifest["openmc_version"], "0.16.0")
            self.assertEqual(
                manifest["inspection"]["source_sha256"], sha256(INSPECTOR)
            )
            self.assertEqual(manifest["inspection"]["numpy_version"], numpy.__version__)
            self.assertEqual(manifest["inspection"]["h5py_version"], h5py.__version__)
            self.assertEqual(manifest["distribution"]["archive_sha256"], sha256(archive))
            self.assertEqual(
                manifest["distribution"]["archive_size_bytes"],
                archive.stat().st_size,
            )
            self.assertEqual(
                manifest["distribution"]["acquisition_profile_sha256"],
                sha256(acquisition_profile),
            )
            self.assertEqual(
                manifest["distribution"]["acquisition_receipt_sha256"],
                sha256(acquisition_receipt),
            )
            self.assertEqual(
                manifest["distribution"]["acquisition_evidence_state"],
                "acquisition_only",
            )
            self.assertEqual(manifest["cross_sections"]["sha256"], sha256(cross_sections))

            boron = next(
                table for table in manifest["neutron_tables"] if table["nuclide"] == "B10"
            )
            self.assertEqual(boron["reactions_mt"], [107, 301])
            self.assertEqual(boron["photon_production_mts"], [107])
            self.assertAlmostEqual(boron["temperatures_k"][0], 293.6)
            self.assertEqual(boron["energy_ranges_ev"], [[1.0e-5, 20.0e6]])
            self.assertEqual(boron["hdf5_version"], [3, 0])

            for table in manifest["photon_tables"]:
                self.assertEqual(table["reactions_mt"], [502, 504, 522])
                self.assertTrue(table["has_atomic_relaxation_data"])
                self.assertTrue(table["has_compton_profile_data"])


if __name__ == "__main__":
    unittest.main()

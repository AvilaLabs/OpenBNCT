"""Cross-language parity tests for the openbnct Python boundary.

Every expectation here is also enforced by the Rust workspace tests; this
suite proves the Python layer observes identical acceptance, rejection,
canonical serialization, and content identity on shared inputs (ADR 0015).
"""

import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import openbnct
from openbnct import NctForgeError

REPO_ROOT = Path(__file__).resolve().parents[3]
TRANSPORT_DIR = REPO_ROOT / "benchmarks" / "synthetic" / "nf-bnct-001" / "transport"


def _response_set_json(qualification: str, with_review: bool) -> str:
    """The same synthetic fixture used by the Rust contract tests."""
    reference = lambda seed: {"id": seed, "sha256": seed * 64}
    document = {
        "schema_version": "openbnct.neutron-response-set/0.1.0",
        "id": "openbnct.synthetic-response-set.v1",
        "qualification": qualification,
        "component_profile": reference("a"),
        "material": reference("b"),
        "nuclear_data_manifest": reference("c"),
        "generation_method": reference("d"),
        "independent_review": reference("e") if with_review else None,
        "transport_energy_range_ev": [0.0, 20.0],
        "energy_ev": [0.0, 1.0, 20.0],
        "unit": "gray_square_centimeter",
        "interpolation": "linear_linear",
        "boron_gy_cm2": [1.0, 2.0, 3.0],
        "nitrogen_gy_cm2": [2.0, 3.0, 4.0],
        "hydrogen_gy_cm2": [3.0, 4.0, 5.0],
        "total_neutron_gy_cm2": [6.0, 9.0, 12.0],
    }
    return json.dumps(document)


class CaseLifecycleTest(unittest.TestCase):
    def test_generate_verify_load_roundtrip(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "nf-bnct-001"
            generated = openbnct.generate_case(root)
            self.assertEqual(generated.ct_file_count, 40)
            self.assertTrue(Path(generated.manifest_file).is_file())

            report = openbnct.verify_case(root)
            self.assertEqual(report.case_id, "NF-BNCT-001")
            self.assertEqual(report.shape, (40, 40, 40))
            self.assertEqual(report.spacing_mm, (5.0, 5.0, 5.0))
            self.assertEqual(report.origin_mm, (-97.5, -97.5, -97.5))
            self.assertEqual(report.ct_slice_count, 40)
            self.assertGreater(report.verified_artifact_count, 40)

            case = openbnct.load_case(root)
            self.assertEqual(case.geometry.shape, (40, 40, 40))
            self.assertEqual(case.geometry.voxel_count, 64_000)
            self.assertEqual(
                case.geometry.direction,
                (1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0),
            )
            self.assertEqual(
                case.geometry.voxel_center_lps_mm(0, 0, 0), (-97.5, -97.5, -97.5)
            )
            self.assertEqual(case.ct_value(0, 0, 0), 0.0)

            rois = {roi.name: roi for roi in case.structures}
            self.assertEqual(
                set(rois),
                {
                    "PHANTOM",
                    "CORE",
                    "LEFT_ANTERIOR_MARKER",
                    "RIGHT_POSTERIOR_MARKER",
                    "CENTRAL_AXIS_2CM",
                },
            )
            self.assertEqual(rois["PHANTOM"].voxel_count, 64_000)
            self.assertEqual(rois["PHANTOM"].volume_cm3, 8_000.0)
            self.assertEqual(rois["CORE"].centroid_lps_mm, (0.0, 0.0, 0.0))
            self.assertEqual(
                rois["LEFT_ANTERIOR_MARKER"].centroid_lps_mm, (70.0, -70.0, -70.0)
            )

            mask = case.structure_mask("CORE")
            self.assertEqual(len(mask), 64_000)
            self.assertEqual(sum(mask), 512)

    def test_generate_refuses_existing_destination(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "nf-bnct-001"
            openbnct.generate_case(root)
            with self.assertRaises(NctForgeError):
                openbnct.generate_case(root)

    def test_verify_rejects_corrupted_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "nf-bnct-001"
            openbnct.generate_case(root)
            target = next((root / "ct").glob("*.dcm"))
            payload = bytearray(target.read_bytes())
            payload[-1] ^= 0xFF
            target.write_bytes(payload)
            with self.assertRaises(NctForgeError):
                openbnct.verify_case(root)
            with self.assertRaises(NctForgeError):
                openbnct.load_case(root)

    def test_verify_rejects_missing_case(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(NctForgeError):
                openbnct.verify_case(Path(tmp) / "absent")


class ManifestTest(unittest.TestCase):
    def test_manifest_binds_verified_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "nf-bnct-001"
            generated = openbnct.generate_case(root)
            manifest = openbnct.read_manifest(generated.manifest_file)

            self.assertEqual(manifest.case_id, "NF-BNCT-001")
            self.assertEqual(manifest.coordinate_system, "dicom_lps_millimeters")
            self.assertEqual(manifest.qualification, "synthetic_research_only")
            self.assertEqual(manifest.geometry.shape, (40, 40, 40))
            self.assertEqual(
                manifest.verify_artifacts(root), len(manifest.artifacts)
            )

            for artifact in manifest.artifacts:
                self.assertEqual(
                    artifact.sha256, openbnct.file_sha256(root / artifact.path)
                )

            # Canonical serialization round-trips through the same contract.
            again = openbnct.read_manifest(
                _write(tmp, "manifest-again.json", manifest.to_json())
            )
            self.assertEqual(again.to_json(), manifest.to_json())

    def test_manifest_rejects_tampered_hash(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "nf-bnct-001"
            generated = openbnct.generate_case(root)
            manifest = openbnct.read_manifest(generated.manifest_file)
            target = next((root / "ct").glob("*.dcm"))
            payload = bytearray(target.read_bytes())
            payload[-1] ^= 0xFF
            target.write_bytes(payload)
            with self.assertRaises(NctForgeError):
                manifest.verify_artifacts(root)


class ContractTest(unittest.TestCase):
    def test_frozen_transport_contracts_load(self) -> None:
        material = openbnct.load_material(TRANSPORT_DIR / "material.json")
        self.assertEqual(material.id, "nctforge.nf-bnct-001.material.v1")

        source = openbnct.load_fixed_source(TRANSPORT_DIR / "source.json")
        self.assertEqual(
            source.schema_version, "openbnct.fixed-source-definition/0.1.0"
        )

        profile = openbnct.load_component_profile(
            TRANSPORT_DIR / "component-profile.json"
        )
        self.assertEqual(profile.id, "nctforge.macroscopic-absorbed-dose.v1")

        method = openbnct.load_response_generation_method(
            TRANSPORT_DIR / "response-generation-method.json"
        )
        self.assertEqual(method.id, "nctforge.nf-bnct-001.response-generation.v1")

    def test_canonical_serialization_is_stable(self) -> None:
        material = openbnct.load_material(TRANSPORT_DIR / "material.json")
        first = material.to_json()
        self.assertEqual(json.loads(first)["id"], material.id)
        with tempfile.TemporaryDirectory() as tmp:
            again = openbnct.load_material(_write(tmp, "material.json", first))
            self.assertEqual(again.to_json(), first)

    def test_contract_rejection_matches_rust(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            document = json.loads(
                (TRANSPORT_DIR / "material.json").read_text()
            )
            document["nuclides"][0]["mass_fraction"] += 1.0
            broken = _write(tmp, "material.json", json.dumps(document))
            with self.assertRaises(NctForgeError):
                openbnct.load_material(broken)

            document = json.loads(
                (TRANSPORT_DIR / "material.json").read_text()
            )
            document["unexpected"] = True
            denied = _write(tmp, "material-denied.json", json.dumps(document))
            with self.assertRaises(NctForgeError):
                openbnct.load_material(denied)


class ResponseSetGateTest(unittest.TestCase):
    def test_unreviewed_set_loads_but_cannot_fold(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = _write(
                tmp,
                "response-set.json",
                _response_set_json("generated_unreviewed", False),
            )
            response_set = openbnct.load_response_set(path)
            self.assertEqual(response_set.qualification, "generated_unreviewed")
            self.assertFalse(response_set.folding_ready)
            self.assertEqual(response_set.energy_knot_count, 3)

    def test_reviewed_set_requires_review_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            missing = _write(
                tmp,
                "response-set.json",
                _response_set_json("independently_reviewed", False),
            )
            with self.assertRaises(NctForgeError):
                openbnct.load_response_set(missing)

            reviewed = _write(
                tmp,
                "response-set-reviewed.json",
                _response_set_json("independently_reviewed", True),
            )
            response_set = openbnct.load_response_set(reviewed)
            self.assertTrue(response_set.folding_ready)


class BackendHonestyTest(unittest.TestCase):
    def test_openmc_capabilities_match_configuration(self) -> None:
        observed = {backend.id: backend for backend in openbnct.backends()}
        self.assertIn("openmc", observed)
        openmc = observed["openmc"]
        # The default backend executes and imports; preparation requires a
        # configured artifact set and stays closed.
        self.assertFalse(openmc.can_prepare)
        self.assertTrue(openmc.can_execute)
        self.assertTrue(openmc.can_import)


class PositioningTest(unittest.TestCase):
    """`aim_source`/`rotate_source` must mirror `openbnct position`."""

    def _geometry(self, tmp: str):
        bundle = openbnct.load_physical_dose_bundle(
            _write(tmp, "dose.json", _physical_bundle_json())
        )
        return bundle.geometry

    def _mask(self, tmp: str) -> Path:
        return _write(
            tmp,
            "mask.json",
            json.dumps({"name": "target", "voxels": [True, True]}),
        )

    def test_aim_positions_source_and_reports(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            geometry = self._geometry(tmp)
            source = openbnct.load_fixed_source(TRANSPORT_DIR / "source.json")
            positioned, report = openbnct.aim_source(
                source,
                geometry,
                self._mask(tmp),
                case_id="synthetic-case",
                approach="-z",
                half_widths_cm=(0.4, 0.2),
                margin_cm=0.1,
            )
            self.assertEqual(report.case_id, "synthetic-case")
            self.assertEqual(report.target_region, "target")
            self.assertEqual(report.entry, ("z", "high"))
            self.assertEqual(report.aperture_half_widths_cm, (0.4, 0.2))
            self.assertAlmostEqual(report.beam_direction_lps[2], -1.0)
            self.assertAlmostEqual(report.target_centroid_lps_mm[0], 0.0)
            self.assertAlmostEqual(report.target_centroid_lps_mm[1], -2.5)
            self.assertAlmostEqual(report.entry_point_lps_mm[2], 0.0)
            self.assertAlmostEqual(report.source_to_centroid_mm, 1.5)
            # The positioned source round-trips through contract validation.
            moved = _write(tmp, "positioned.json", positioned.to_json())
            self.assertEqual(
                openbnct.load_fixed_source(moved).to_json(),
                positioned.to_json(),
            )
            with tempfile.TemporaryDirectory() as out:
                written = Path(out) / "report.json"
                report.write(written)
                self.assertEqual(
                    json.loads(written.read_text())["entry_axis"], "z"
                )

    def test_aim_requires_an_axis_or_direction(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            source = openbnct.load_fixed_source(TRANSPORT_DIR / "source.json")
            with self.assertRaises(ValueError):
                openbnct.aim_source(
                    source, self._geometry(tmp), self._mask(tmp), "case"
                )
            with self.assertRaises(ValueError):
                openbnct.aim_source(
                    source,
                    self._geometry(tmp),
                    self._mask(tmp),
                    "case",
                    approach="diagonal",
                )

    def test_rotate_source_by_quarter_turn(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            geometry = self._geometry(tmp)
            source = openbnct.load_fixed_source(TRANSPORT_DIR / "source.json")
            positioned, _ = openbnct.aim_source(
                source,
                geometry,
                self._mask(tmp),
                case_id="synthetic-case",
                approach="-z",
                half_widths_cm=(0.4, 0.2),
            )
            rotated = openbnct.rotate_source(
                positioned, "x", (0.0, -2.5, -2.5), 90.0
            )
            self.assertNotEqual(rotated.to_json(), positioned.to_json())
            moved = _write(tmp, "rotated.json", rotated.to_json())
            self.assertEqual(
                openbnct.load_fixed_source(moved).to_json(), rotated.to_json()
            )
            with self.assertRaises(NctForgeError):
                openbnct.rotate_source(positioned, "x", (0.0, 0.0, 0.0), 45.0)
            with self.assertRaises(ValueError):
                openbnct.rotate_source(positioned, "w", (0.0, 0.0, 0.0), 90.0)


def _physical_bundle_json() -> str:
    """The same synthetic fixture used by the Rust bio/evidence tests."""
    reference = lambda seed: {"id": seed, "sha256": seed * (64 // len(seed))}
    component = lambda name, mean, sigma: {
        "component": name,
        "unit": "gray_per_source_particle",
        "values": [mean, mean],
        "absolute_standard_uncertainty": [sigma, sigma],
    }
    return json.dumps(
        {
            "schema_version": "openbnct.physical-dose-bundle/0.2.0",
            "case_id": "synthetic-case",
            "frame_of_reference_uid": None,
            "geometry": {
                "shape": [2, 1, 1],
                "spacing_mm": [5.0, 5.0, 5.0],
                "origin_mm": [-2.5, -2.5, -2.5],
                "direction": [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            },
            "component_profile": reference("ab"),
            "response_set": reference("cd"),
            "components": [
                component("boron", 1.0e-12, 1.0e-14),
                component("nitrogen", 2.0e-13, 2.0e-15),
                component("hydrogen", 5.0e-14, 5.0e-16),
                component("photon", 3.0e-13, 3.0e-15),
            ],
            "physical_total": {
                "unit": "gray_per_source_particle",
                "values": [1.75e-12, 1.75e-12],
                "absolute_standard_uncertainty": [1.1e-14, 1.1e-14],
                "uncertainty_method": "dedicated_estimator",
            },
            "provenance_id": "test-provenance",
        }
    )


def _model_json() -> str:
    return json.dumps(
        {
            "schema_version": "openbnct.biological-model/0.2.0",
            "id": "openbnct.tests.fixed-weights.v1",
            "weight_semantics": "fixed_per_component",
            "input_unit": "gray_per_source_particle",
            "component_weights": {
                "boron": 3.8,
                "nitrogen": 2.5,
                "hydrogen": 1.0,
                "photon": 1.0,
            },
        }
    )


class DoseBundleTest(unittest.TestCase):
    def test_load_validate_and_histogram(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = _write(tmp, "dose.json", _physical_bundle_json())
            bundle = openbnct.load_physical_dose_bundle(path)
            self.assertEqual(bundle.case_id, "synthetic-case")
            self.assertEqual(bundle.provenance_id, "test-provenance")
            self.assertEqual(len(bundle.components), 4)
            self.assertEqual(bundle.physical_total.values, [1.75e-12, 1.75e-12])

            dvh = openbnct.compute_dvh(
                bundle, "component:boron", "all", [True, True], 4
            )
            self.assertEqual(dvh.unit, "gray_per_source_particle")
            self.assertEqual(dvh.region_voxel_count, 2)
            self.assertAlmostEqual(
                sum(dvh.differential_volume_fraction), 1.0, places=9
            )
            self.assertEqual(dvh.cumulative_volume_fraction[0], 1.0)

    def test_rejects_broken_bundle(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            document = json.loads(_physical_bundle_json())
            document["components"][0]["values"] = [1.0]
            with self.assertRaises(NctForgeError):
                openbnct.load_physical_dose_bundle(
                    _write(tmp, "bad.json", json.dumps(document))
                )


class BiologicalLayerTest(unittest.TestCase):
    def test_apply_and_histogram_biological_total(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bundle_path = _write(tmp, "dose.json", _physical_bundle_json())
            model_path = _write(tmp, "model.json", _model_json())
            bundle = openbnct.load_physical_dose_bundle(bundle_path)
            model = openbnct.load_biological_model(model_path)
            biological = openbnct.apply_model(model, bundle, [])
            self.assertEqual(biological.unit, "weighted_gray_per_source_particle")
            self.assertEqual(
                biological.qualification, "synthetic_research_only_not_clinical"
            )
            self.assertEqual(
                biological.physical_bundle_provenance, "test-provenance"
            )
            boron = next(
                c for c in biological.components if c.component == "boron"
            )
            self.assertEqual(boron.values, [3.8e-12, 3.8e-12])
            # Total sigma is the correlated sum of weighted component sigmas.
            expected_sigma = 3.8e-14 + 2.5 * 2.0e-15 + 5.0e-16 + 3.0e-15
            self.assertAlmostEqual(
                biological.biological_total.absolute_standard_uncertainty[0],
                expected_sigma,
            )

            dvh = openbnct.compute_dvh_biological(
                biological, "biological_total", "all", [True, True], 4
            )
            self.assertEqual(dvh.unit, "weighted_gray_per_source_particle")

            out = Path(tmp) / "bio.json"
            biological.write(out)
            reloaded = json.loads(out.read_text())
            self.assertEqual(
                reloaded["schema_version"],
                "openbnct.biological-dose-bundle/0.2.0",
            )

    def test_region_mask_name_must_match(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bundle = openbnct.load_physical_dose_bundle(
                _write(tmp, "dose.json", _physical_bundle_json())
            )
            model_document = json.loads(_model_json())
            model_document["region_weights"] = {
                "core": model_document["component_weights"]
            }
            model = openbnct.load_biological_model(
                _write(tmp, "model.json", json.dumps(model_document))
            )
            mask = _write(
                tmp,
                "mask.json",
                json.dumps({"name": "other", "voxels": [True, False]}),
            )
            with self.assertRaises(NctForgeError):
                openbnct.apply_model(model, bundle, [("core", mask)])

    def test_sweep_biological_model_component_weight(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bundle_path = _write(tmp, "dose.json", _physical_bundle_json())
            model_path = _write(tmp, "model.json", _model_json())
            mask_path = _write(
                tmp, "mask.json", json.dumps({"name": "all", "voxels": [True, True]})
            )
            bundle = openbnct.load_physical_dose_bundle(bundle_path)
            model = openbnct.load_biological_model(model_path)
            sweep = openbnct.sweep_biological_model(
                model,
                bundle,
                [("all", mask_path)],
                "all",
                "component:boron",
                [0.0, 3.8],
            )
            self.assertEqual(
                sweep.schema_version, "openbnct.bio-sensitivity-sweep/0.1.0"
            )
            self.assertEqual(sweep.parameter, "component:boron")
            self.assertEqual(sweep.quantity, "biological_total")
            self.assertEqual(len(sweep.points), 2)
            # boron 0 -> 8.5e-13; boron 3.8 -> 4.65e-12 (2 voxels, uniform).
            self.assertAlmostEqual(sweep.points[0][3], 8.5e-13)
            self.assertAlmostEqual(sweep.points[1][3], 4.65e-12)
            out = Path(tmp) / "sweep.json"
            sweep.write(out)
            reloaded = json.loads(out.read_text())
            self.assertEqual(reloaded["case_id"], "synthetic-case")

            with self.assertRaises(NctForgeError):
                openbnct.sweep_biological_model(
                    model, bundle, [("all", mask_path)], "all", "bogus", [1.0]
                )
            with self.assertRaises(NctForgeError):
                openbnct.sweep_biological_model(
                    model,
                    bundle,
                    [("all", mask_path)],
                    "missing-region",
                    "component:boron",
                    [1.0],
                )

    def test_make_biological_model_external_experiment(self) -> None:
        """An externally-authored model dict validates and applies in-place."""
        with tempfile.TemporaryDirectory() as tmp:
            bundle = openbnct.load_physical_dose_bundle(
                _write(tmp, "dose.json", _physical_bundle_json())
            )
            model = openbnct.make_biological_model(
                {
                    "schema_version": "openbnct.biological-model/0.2.0",
                    "id": "experiment.custom-weights",
                    "weight_semantics": "fixed_per_component",
                    "input_unit": "gray_per_source_particle",
                    "component_weights": {
                        "boron": 5.0,
                        "nitrogen": 1.0,
                        "hydrogen": 1.0,
                        "photon": 1.0,
                    },
                }
            )
            self.assertEqual(model.id, "experiment.custom-weights")
            biological = openbnct.apply_model(model, bundle, [])
            boron = next(
                c for c in biological.components if c.component == "boron"
            )
            self.assertEqual(boron.values, [5.0e-12, 5.0e-12])
            # A model failing the shared validate() rejects identically.
            with self.assertRaises(NctForgeError):
                openbnct.make_biological_model(
                    {
                        "schema_version": "openbnct.biological-model/0.2.0",
                        "id": "experiment.bad",
                        "weight_semantics": "photon_isoeffective",
                        "input_unit": "gray_per_source_particle",
                        "component_weights": {
                            "boron": 5.0,
                            "nitrogen": 1.0,
                            "hydrogen": 1.0,
                            "photon": 2.0,  # isoeffective requires exactly 1.0
                        },
                    }
                )

    def test_photon_isoeffective_fractionated_total(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bundle = openbnct.load_physical_dose_bundle(
                _write(tmp, "dose.json", _physical_bundle_json())
            )
            model_document = json.loads(_model_json())
            model_document["weight_semantics"] = "photon_isoeffective"
            model_document["fractionation"] = {
                "fraction_count": 30,
                "source_particles_per_fraction": 1.0e12,
                "default_alpha_beta": 3.0,
            }
            model = openbnct.load_biological_model(
                _write(tmp, "model.json", json.dumps(model_document))
            )
            biological = openbnct.apply_model(model, bundle, [])
            self.assertEqual(biological.weight_semantics, "photon_isoeffective")
            # Weighted per-particle total w = 3.8e-12 + 2.5*2e-13 + 1*5e-14 +
            # 1*3e-13 = 4.65e-12; d = w * 1e12 = 4.65; EQD2 with n=30, r=3.
            d = 4.65
            expected = 30.0 * d * (1.0 + d / 3.0) / (1.0 + 2.0 / 3.0)
            self.assertAlmostEqual(
                biological.biological_total.values[0],
                expected,
                delta=expected * 1e-12,
            )
            self.assertEqual(
                biological.biological_total.unit, "weighted_eqd2"
            )

    def test_photon_isoeffective_rejects_non_unit_photon_weight(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            model_document = json.loads(_model_json())
            model_document["weight_semantics"] = "photon_isoeffective"
            model_document["component_weights"]["photon"] = 1.4
            with self.assertRaises(NctForgeError):
                openbnct.load_biological_model(
                    _write(tmp, "model.json", json.dumps(model_document))
                )


class MetricsAndEndpointTest(unittest.TestCase):
    def _endpoint_model_json(self) -> str:
        return json.dumps(
            {
                "schema_version": "openbnct.endpoint-model/0.1.0",
                "id": "openbnct.tests.logistic-tcp.v1",
                "endpoint": "tcp",
                "function": {
                    "kind": "logistic",
                    "d50": 2.0e-12,
                    "gamma50": 2.0,
                },
                "dose_statistic": {"statistic": "mean"},
            }
        )

    def test_metrics_match_rust_semantics(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bundle = openbnct.load_physical_dose_bundle(
                _write(tmp, "dose.json", _physical_bundle_json())
            )
            metrics = openbnct.compute_metrics(
                bundle,
                "physical_total",
                "all",
                [True, True],
                [50.0, 100.0],
                [1.75e-12, 2.0e-12],
                [1.0, 10.0],
            )
            self.assertEqual(metrics.unit, "gray_per_source_particle")
            self.assertEqual(metrics.minimum_dose, metrics.maximum_dose)
            self.assertEqual(metrics.mean_dose, 1.75e-12)
            # Uniform two-voxel volume: D100 = D50 = the single dose level.
            self.assertEqual(metrics.dx, [(50.0, 1.75e-12), (100.0, 1.75e-12)])
            self.assertEqual(metrics.vx, [(1.75e-12, 1.0), (2.0e-12, 0.0)])
            self.assertEqual(metrics.eud[0][1], 1.75e-12)  # a=1 is the mean
            self.assertEqual(metrics.schema_version,
                             "openbnct.dose-metrics/0.1.0")

    def test_endpoint_evaluate_and_utcp(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bundle = openbnct.load_physical_dose_bundle(
                _write(tmp, "dose.json", _physical_bundle_json())
            )
            model = openbnct.load_endpoint_model(
                _write(tmp, "model.json", self._endpoint_model_json())
            )
            tcp = openbnct.evaluate_endpoint(
                model, bundle, "physical_total", "all", [True, True]
            )
            self.assertEqual(tcp.endpoint, "tcp")
            # Mean dose 1.75e-12 with d50 = 2e-12, gamma50 = 2:
            # P = 1/(1 + (2/1.75)^8).
            expected = 1.0 / (1.0 + (2.0 / 1.75) ** 8)
            self.assertAlmostEqual(tcp.probability, expected, delta=1e-9)
            self.assertEqual(tcp.dose_statistic.kind, "mean")

            ntcp_document = json.loads(self._endpoint_model_json())
            ntcp_document["endpoint"] = "ntcp"
            ntcp_document["function"] = {
                "kind": "probit",
                "td50": 1.75e-12,
                "m": 0.3,
            }
            ntcp_model = openbnct.load_endpoint_model(
                _write(tmp, "ntcp.json", json.dumps(ntcp_document))
            )
            ntcp = openbnct.evaluate_endpoint(
                ntcp_model, bundle, "physical_total", "all", [True, True]
            )
            # Mean dose = td50 -> probit is ~0.5 within approximation bound.
            self.assertAlmostEqual(ntcp.probability, 0.5, delta=1e-6)

            utcp = openbnct.combine_utcp(tcp, ntcp, "p_plus")
            self.assertEqual(utcp.endpoint, "utcp")
            self.assertAlmostEqual(
                utcp.probability,
                tcp.probability * (1.0 - ntcp.probability),
                delta=1e-12,
            )
            self.assertIsNone(utcp.dose_statistic)
            self.assertEqual(
                utcp.qualification, "synthetic_research_only_not_clinical"
            )

            # Two TCP evaluations cannot be combined.
            with self.assertRaises(NctForgeError):
                openbnct.combine_utcp(tcp, tcp, "p_plus")


class ExposurePlanTest(unittest.TestCase):
    def test_table_round_trip_and_accumulate(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bundle_path = _write(tmp, "dose.json", _physical_bundle_json())
            digest = openbnct.file_sha256(bundle_path)
            exposure = {
                "name": "field-a",
                "dose_bundle": {
                    "id": "dose",
                    "sha256": digest,
                    "path": "dose.json",
                },
                "weight": 1.5,
                "weight_basis": "delivered_fraction",
                "duration_s": 600.0,
                "boron_assumption": "10 ppm B-10",
            }
            plan = {
                "schema_version": "openbnct.exposure-plan/0.1.0",
                "id": "openbnct.test.plan.v1",
                "case_id": "accumulated-case",
                "covariance": "independent_exposures",
                "exposures": [exposure],
            }
            plan_path = _write(tmp, "plan.json", json.dumps(plan))
            loaded = openbnct.load_exposure_plan(plan_path)
            self.assertEqual(loaded.id, "openbnct.test.plan.v1")
            self.assertEqual(loaded.exposures[0].weight_basis, "delivered_fraction")
            self.assertEqual(loaded.validate_diagnostics(), [])

            # Accumulation through Python returns the same bundle the CLI
            # `accumulate` command writes (1.5x dose on the fixture).
            accumulated = openbnct.accumulate_exposures(plan_path)
            self.assertEqual(accumulated.case_id, "accumulated-case")
            self.assertAlmostEqual(
                accumulated.physical_total.values[0], 1.75e-12 * 1.5, places=18
            )

            # Table export -> import round-trips through both surfaces.
            csv_path = Path(tmp) / "schedule.csv"
            xlsx_path = Path(tmp) / "schedule.xlsx"
            openbnct.plan_table_write(plan_path, csv_path)
            openbnct.plan_table_write(plan_path, xlsx_path)
            from_csv = Path(tmp) / "from-csv.json"
            from_xlsx = Path(tmp) / "from-xlsx.json"
            openbnct.plan_table_read(csv_path, from_csv)
            openbnct.plan_table_read(xlsx_path, from_xlsx)
            self.assertEqual(
                json.loads(from_csv.read_text()), json.loads(plan_path.read_text())
            )
            self.assertEqual(
                json.loads(from_xlsx.read_text()), json.loads(plan_path.read_text())
            )

    def test_diagnostics_report_every_issue(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            plan = {
                "schema_version": "openbnct.exposure-plan/0.1.0",
                "id": "  ",
                "case_id": "case",
                "covariance": "independent_exposures",
                "exposures": [
                    {
                        "name": "a",
                        "dose_bundle": {"id": "x", "sha256": "bad", "path": "x"},
                        "weight": -1.0,
                        "weight_basis": "manual",
                        "duration_s": None,
                        "boron_assumption": None,
                    }
                ],
            }
            path = _write(tmp, "bad.json", json.dumps(plan))
            issues = openbnct.exposure_plan_diagnostics(path)
            # empty id, negative weight, malformed sha256 — all reported.
            self.assertEqual(len(issues), 3)
            # The strict loader still refuses the same document.
            with self.assertRaises(NctForgeError):
                openbnct.load_exposure_plan(path)


class InterchangeTest(unittest.TestCase):
    def test_imports_external_component_dose(self) -> None:
        document = (
            REPO_ROOT / "examples" / "interchange" / "phits-synthetic-dose.json"
        )
        bundle = openbnct.import_component_dose(document)
        self.assertEqual(bundle.case_id, "nf-bnct-001-phits-synthetic")
        self.assertEqual(bundle.physical_total.unit, "gray_per_source_particle")
        # component_sum imports never claim a total uncertainty.
        self.assertIsNone(bundle.physical_total.absolute_standard_uncertainty)
        self.assertIn("interchange:phits:sha256:", bundle.provenance_id)
        self.assertEqual(len(bundle.components), 4)
        # The component sum equals the physical total (tolerance for f64 sum).
        for component in bundle.components:
            self.assertEqual(len(component.values), 64000)
        summed = [sum(c.values[i] for c in bundle.components) for i in (0, 1000)]
        for index, expected in zip((0, 1000), summed):
            self.assertAlmostEqual(
                bundle.physical_total.values[index], expected, places=18
            )

    def test_rejects_malformed_interchange(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            doc = json.loads(
                (
                    REPO_ROOT
                    / "examples"
                    / "interchange"
                    / "phits-synthetic-dose.json"
                ).read_text()
            )
            doc["components"].pop()  # drop photon — a required component
            path = _write(tmp, "broken.json", json.dumps(doc))
            with self.assertRaises(NctForgeError):
                openbnct.import_component_dose(path)


MESHTAL_FIXTURE = """mcnp   version 6.2 ld=01/01/20  probid =  01/01/20 00:00:00
 parity fixture
 Number of histories used for normalizing tallies =    1000.00

 Mesh Tally Number        4
 neutron  mesh tally.

 Tally bin boundaries:
    X direction:      -1.00      1.00
    Y direction:      -1.00      0.00      1.00
    Z direction:      -1.00      1.00
    Energy bin boundaries: 0.00E+00 2.00E+01

   Energy         X         Y         Z     Result     Rel Error
  2.000E+01     0.000   -0.500     0.000 1.00000E-03 1.00000E-02
  2.000E+01     0.000    0.500     0.000 2.00000E-03 1.00000E-02
"""

PHITS_FIXTURE = """[ T - D e p o s i t ]
    mesh =  xyz
  x-type =    2
    xmin =  -1.0
    xmax =   1.0
      nx =    2
  y-type =    2
    ymin =  -1.0
    ymax =   1.0
      ny =    2
  z-type =    2
    zmin =  -1.0
    zmax =   1.0
      nz =    2
    unit =    0
    axis =   xy
    file = d.out
   output =  dose

#newpage:
# no. = 1  iz = 1
x: x [cm]
y: y [cm]
h: x n n y n y(all),hh0l n
# x-lower      x-upper      y-lower      y-upper      all        r.err
 -1.0000E+00   0.0000E+00  -1.0000E+00   0.0000E+00   1.0000E-03 1.0000E-02
  0.0000E+00   1.0000E+00  -1.0000E+00   0.0000E+00   2.0000E-03 1.0000E-02
 -1.0000E+00   0.0000E+00   0.0000E+00   1.0000E+00   4.0000E-03 1.0000E-02
  0.0000E+00   1.0000E+00   0.0000E+00   1.0000E+00   5.0000E-03 1.0000E-02

#newpage:
# no. = 2  iz = 2
x: x [cm]
y: y [cm]
h: x n n y n y(all),hh0l n
# x-lower      x-upper      y-lower      y-upper      all        r.err
 -1.0000E+00   0.0000E+00  -1.0000E+00   0.0000E+00   7.0000E-03 2.0000E-02
  0.0000E+00   1.0000E+00  -1.0000E+00   0.0000E+00   8.0000E-03 2.0000E-02
 -1.0000E+00   0.0000E+00   0.0000E+00   1.0000E+00   9.0000E-03 2.0000E-02
  0.0000E+00   1.0000E+00   0.0000E+00   1.0000E+00   1.0000E-02 2.0000E-02
"""


class ExternalDoseTest(unittest.TestCase):
    """OP-10: external-dose import, BED conversion, and combined evaluation."""

    def _external_doc(self, shape: list[int] | None = None) -> dict:
        geometry = json.loads(
            (REPO_ROOT / "examples" / "interchange" / "phits-synthetic-dose.json")
            .read_text()
        )["geometry"]
        if shape is not None:
            geometry = dict(geometry, shape=shape)
        voxels = 1
        for dim in geometry["shape"]:
            voxels *= dim
        return {
            "schema_version": "openbnct.external-dose/0.1.0",
            "case_id": "nf-bnct-001-phits-synthetic",
            "geometry": geometry,
            "producer": {
                "system": "photon-course-sim",
                "version": "research-1",
                "normalization": "absolute gray",
            },
            "quantity": "physical",
            "values": [60.0] * voxels,
            "absolute_standard_uncertainty": [0.6] * voxels,
            "fractionation": {"kind": "uniform", "count": 30},
        }

    def _bnct_eqd2_bundle(self) -> openbnct.BiologicalDoseBundle:
        physical = openbnct.import_component_dose(
            REPO_ROOT / "examples" / "interchange" / "phits-synthetic-dose.json"
        )
        model = openbnct.load_biological_model(
            REPO_ROOT
            / "examples"
            / "biological"
            / "photon-isoeffective-lq-model-v1.json"
        )
        return openbnct.apply_model(
            model,
            physical,
            [("core", REPO_ROOT / "examples" / "biological" / "core-region-mask.json")],
        )

    def test_import_external_dose(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = _write(tmp, "ext.json", json.dumps(self._external_doc()))
            bundle = openbnct.import_external_dose(path)
            self.assertEqual(bundle.case_id, "nf-bnct-001-phits-synthetic")
            self.assertEqual(bundle.quantity, "physical")
            self.assertEqual(bundle.fractions, 30)
            self.assertIn("external-dose:photon-course-sim:sha256:", bundle.provenance_id)

    def test_bed_and_combine(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = _write(tmp, "ext.json", json.dumps(self._external_doc()))
            dose = openbnct.import_external_dose(path)
            # 60 Gy in 30 fx at r=3: d=2, BED=30·2·(1+2/3)=100, EQD2=100/(5/3)=60.
            eqd2 = openbnct.bed_from_external_dose(dose, alpha_beta=3.0)
            self.assertEqual(eqd2.quantity, "eqd2")
            self.assertAlmostEqual(eqd2.values[0], 60.0, places=9)
            bed = openbnct.bed_from_external_dose(dose, alpha_beta=3.0, quantity="bed")
            self.assertAlmostEqual(bed.values[0], 100.0, places=9)

            combined = openbnct.combine_biological_doses(
                self._bnct_eqd2_bundle(),
                eqd2,
                assumption="full-repair additive EQD2; independent courses",
            )
            self.assertEqual(combined.quantity, "eqd2")
            self.assertEqual(len(combined.inputs), 2)
            roles = {role for role, _, _, _ in combined.inputs}
            self.assertEqual(roles, {"bnct_biological", "external_course"})
            self.assertIn("full-repair", combined.additivity_assumption)
            self.assertIsNone(combined.external_resampling)

    def test_combine_rejects_incompatible_quantity(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = _write(tmp, "ext.json", json.dumps(self._external_doc()))
            dose = openbnct.import_external_dose(path)
            bed = openbnct.bed_from_external_dose(dose, alpha_beta=3.0, quantity="bed")
            with self.assertRaises(NctForgeError):
                openbnct.combine_biological_doses(
                    self._bnct_eqd2_bundle(), bed, assumption="x"
                )
            # Empty assumption is never accepted.
            eqd2 = openbnct.bed_from_external_dose(dose, alpha_beta=3.0)
            with self.assertRaises(NctForgeError):
                openbnct.combine_biological_doses(self._bnct_eqd2_bundle(), eqd2)

    def test_combine_rejects_uncovered_grid_without_resample(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            doc = self._external_doc(shape=[80, 80, 80])
            path = _write(tmp, "ext-big.json", json.dumps(doc))
            dose = openbnct.import_external_dose(path)
            eqd2 = openbnct.bed_from_external_dose(dose, alpha_beta=3.0)
            with self.assertRaises(NctForgeError):
                openbnct.combine_biological_doses(
                    self._bnct_eqd2_bundle(), eqd2, assumption="x"
                )


class DoseComparisonTest(unittest.TestCase):
    def test_compare_identical_and_reject_mismatched_case(self) -> None:
        bundle = openbnct.import_component_dose(
            REPO_ROOT / "examples" / "interchange" / "phits-synthetic-dose.json"
        )
        cmp = openbnct.compare_dose_bundles(bundle, bundle)
        self.assertEqual(cmp.schema_version, "openbnct.dose-comparison/0.1.0")
        self.assertEqual(cmp.voxel_count, 64000)
        self.assertEqual(len(cmp.quantities), 5)
        for (_, _, max_abs, _, _, max_norm, within) in cmp.quantities:
            self.assertEqual(max_abs, 0.0)
            self.assertEqual(max_norm, 0.0)
            # component_sum imports carry no total sigma — total reports None.
        roles = {role for role, _, _, _ in cmp.inputs}
        self.assertEqual(roles, {"reference", "candidate"})
        # A different case_id can never be compared on this path.
        with tempfile.TemporaryDirectory() as tmp:
            doc = json.loads(
                (
                    REPO_ROOT
                    / "examples"
                    / "interchange"
                    / "phits-synthetic-dose.json"
                ).read_text()
            )
            doc["case_id"] = "other-case"
            other = openbnct.import_component_dose(
                _write(tmp, "other.json", json.dumps(doc))
            )
            with self.assertRaises(NctForgeError):
                openbnct.compare_dose_bundles(bundle, other)


class ExternalAdapterTest(unittest.TestCase):
    def test_mcnp_meshtal_import(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = _write(tmp, "meshtal", MESHTAL_FIXTURE)
            bundle = openbnct.import_mcnp_meshtal(
                {
                    "boron": (path, 4),
                    "nitrogen": (path, 4),
                    "hydrogen": (path, 4),
                    "photon": (path, 4),
                },
                case_id="py-mcnp",
                unit="gray_per_source_particle",
                normalization="per source particle; fixture",
            )
            self.assertEqual(bundle.case_id, "py-mcnp")
            self.assertIn("interchange:mcnp:sha256:", bundle.provenance_id)
            # 1x2x1 grid: values = 4 x 1e-3 at index 0, 4 x 2e-3 at index 1.
            self.assertAlmostEqual(bundle.physical_total.values[0], 4.0e-3)
            self.assertAlmostEqual(bundle.physical_total.values[1], 8.0e-3)
            self.assertIsNone(bundle.physical_total.absolute_standard_uncertainty)

    def test_phits_import(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = _write(tmp, "d.out", PHITS_FIXTURE)
            bundle = openbnct.import_phits(
                {
                    "boron": path,
                    "nitrogen": path,
                    "hydrogen": path,
                    "photon": path,
                },
                case_id="py-phits",
                unit="gray_per_source_particle",
                normalization="unit=0; fixture",
                producer_version="3.34",
            )
            self.assertIn("interchange:phits:sha256:", bundle.provenance_id)
            # 2x2x2: index 7 = (1,1,1) = 4 x 1e-2.
            self.assertAlmostEqual(bundle.physical_total.values[7], 4.0e-2)

    def test_export_mcnp_deck(self) -> None:
        case = REPO_ROOT / "benchmarks/synthetic/nf-bnct-001/transport/case.json"
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "deck.i"
            deck = openbnct.export_mcnp_deck(case, out, xs_suffix="80c", seed=42)
            self.assertEqual(deck, out.read_text())
            self.assertIn("mode n p", deck)
            self.assertIn("fmesh4:n geom=xyz", deck)
            self.assertIn("fmesh14:p", deck)
            self.assertIn("m1 1001.80c", deck)
            self.assertIn("5010.80c", deck)
            self.assertIn("nps 1000", deck)
            self.assertIn("rand seed=42", deck)
            self.assertIn("sha256:", deck)
            # No overwrite of an existing deck.
            with self.assertRaises(NctForgeError):
                openbnct.export_mcnp_deck(case, out, xs_suffix="80c")

    def test_adapters_reject_bad_specs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = _write(tmp, "meshtal", MESHTAL_FIXTURE)
            with self.assertRaises(NctForgeError):
                openbnct.import_mcnp_meshtal(
                    {"boron": (path, 99), "nitrogen": (path, 4),
                     "hydrogen": (path, 4), "photon": (path, 4)},
                    "c", "gray_per_source_particle", "x",
                )
            with self.assertRaises(NctForgeError):
                openbnct.import_phits(
                    {"boron": path, "nitrogen": path,
                     "hydrogen": path, "photon": path},
                    "c", "gray_per_source_particle", "x", " ",
                )


class EvidenceBundleTest(unittest.TestCase):
    def test_verify_detects_tampering(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            payload = root / "payload.json"
            payload.write_text('{"ok": true}')
            digest = hashlib.sha256(payload.read_bytes()).hexdigest()
            (root / "artifact-manifest.json").write_text(
                json.dumps(
                    {
                        "schema_version": "openbnct.evidence-bundle-manifest/0.1.0",
                        "case_id": "synthetic-case",
                        "qualification": "synthetic_research_only",
                        "artifacts": [
                            {
                                "role": "payload",
                                "path": "payload.json",
                                "sha256": digest,
                                "media_type": None,
                            }
                        ],
                    }
                )
            )
            case_id, count = openbnct.verify_evidence_bundle(root)
            self.assertEqual((case_id, count), ("synthetic-case", 1))
            payload.write_text('{"ok": false}')
            with self.assertRaises(NctForgeError):
                openbnct.verify_evidence_bundle(root)


def _write(directory: str, name: str, content: str) -> Path:
    path = Path(directory) / name
    path.write_text(content)
    return path


def _nifti_volume(directory: str, name: str, values: list[float]) -> Path:
    """Write a minimal NIfTI-1 single file: 2x2x1 float64 on an
    axis-aligned 1 mm grid at the LPS origin (RAS affine diag(-1,-1,1))."""
    import struct

    header = bytearray(352)
    struct.pack_into("<i", header, 0, 348)          # sizeof_hdr
    struct.pack_into("<4h", header, 40, 3, 2, 2, 1)  # dim[0..3]
    struct.pack_into("<h", header, 70, 64)           # float64
    struct.pack_into("<h", header, 72, 64)           # bitpix
    struct.pack_into("<4f", header, 76, 1.0, 1.0, 1.0, 1.0)  # pixdim
    struct.pack_into("<f", header, 108, 352.0)       # vox_offset
    struct.pack_into("<f", header, 112, 1.0)         # scl_slope
    struct.pack_into("<h", header, 254, 1)           # sform_code
    for row, (vals, off) in enumerate(
        zip(((-1.0, 0.0, 0.0, 0.0), (0.0, -1.0, 0.0, 0.0), (0.0, 0.0, 1.0, 0.0)),
            (280, 296, 312))
    ):
        struct.pack_into("<4f", header, off, *vals)
    header[123] = 0b010                              # xyzt: mm
    header[344:348] = b"n+1\0"
    path = Path(directory) / name
    path.write_bytes(bytes(header) + struct.pack(f"<{len(values)}d", *values))
    return path


class NiftiImportTest(unittest.TestCase):
    def test_nifti_component_import(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            comps = {}
            for i, name in enumerate(("boron", "nitrogen", "hydrogen", "photon")):
                value = _nifti_volume(tmp, f"{name}.nii", [i + 1.0] * 4)
                sigma = _nifti_volume(tmp, f"{name}_s.nii", [0.1] * 4)
                comps[name] = (value, sigma)
            bundle = openbnct.import_nifti(
                comps,
                case_id="py-nifti",
                unit="gray_per_source_particle",
                normalization="per source neutron; fixture",
                producer_system="openpint",
                producer_version="test",
            )
            self.assertEqual(bundle.case_id, "py-nifti")
            self.assertIn("interchange:openpint:sha256:", bundle.provenance_id)
            # components 1+2+3+4 per voxel (component order is not stable)
            self.assertAlmostEqual(bundle.physical_total.values[0], 10.0)
            self.assertEqual(len(bundle.components), 4)
            self.assertTrue(
                all(
                    c.absolute_standard_uncertainty[0] == 0.1
                    for c in bundle.components
                )
            )

    def test_nifti_import_requires_producer_and_four_components(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            f = _nifti_volume(tmp, "a.nii", [1.0] * 4)
            comps = {"boron": f, "nitrogen": f, "hydrogen": f, "photon": f}
            with self.assertRaises(NctForgeError):
                openbnct.import_nifti(
                    comps, "c", "gray_per_source_particle", "x", " "
                )
            with self.assertRaises(NctForgeError):
                openbnct.import_nifti(
                    {"boron": f}, "c", "gray_per_source_particle", "x", "openpint"
                )


if __name__ == "__main__":
    unittest.main()

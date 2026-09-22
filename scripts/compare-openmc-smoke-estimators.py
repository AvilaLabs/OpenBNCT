#!/usr/bin/env python3
# SPDX-License-Identifier: MIT

"""Compare an OpenMC smoke statepoint against its bound estimator contract."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
from pathlib import Path
import platform
import re
from typing import Any

try:
    import h5py
    import numpy
except ModuleNotFoundError:
    raise SystemExit(
        "error: install scripts/requirements-openmc-data-inspector.txt first"
    ) from None


REPORT_SCHEMA = "openbnct.openmc-smoke-estimator-comparison/0.1.0"
COMPARISON_METHOD = "openbnct-openmc-smoke-estimator-comparator/0.1.0"
INPUT_MANIFEST_SCHEMA = "openbnct.openmc-input-manifest/0.1.0"
RESPONSE_SET_SCHEMA = "openbnct.neutron-response-set/0.1.0"
MATERIAL_SCHEMA_PREFIX = "openbnct."
EXECUTION_PROFILE_SCHEMA = "openbnct.openmc-execution-profile/0.1.0"
EXECUTION_RECEIPT_SCHEMA = "openbnct.njoy-execution-receipt/0.1.0"
TARGET_OPENMC_VERSION = "0.16.0"
TARGET_NJOY_VERSION = "2016.78"
EV_PER_JOULE = 1.0 / 1.602176634e-19
JOULE_PER_EV = 1.602176634e-19
COMPONENT_TALLIES = {
    "boron": "openbnct.component.boron.response",
    "nitrogen": "openbnct.component.nitrogen.response",
    "hydrogen": "openbnct.component.hydrogen.response",
}
COMPONENT_CURVES = {
    "boron": "boron_gy_cm2",
    "nitrogen": "nitrogen_gy_cm2",
    "hydrogen": "hydrogen_gy_cm2",
}
NEUTRON_HEATING_TALLY = "openbnct.audit.neutron_heating"
PHOTON_HEATING_TALLY = "openbnct.component.photon.heating"
COUPLED_HEATING_TALLY = "openbnct.physical_total.coupled_heating"
NEUTRON_FLUENCE_TALLY = "openbnct.diagnostic.neutron_fluence"
AUDIT_REACTIONS = {
    "boron": {"tally": "openbnct.audit.b10_mt107", "nuclide": "B10", "xs_mt": 107, "kerma_mt": 407},
    "nitrogen": {"tally": "openbnct.audit.n14_mt103", "nuclide": "N14", "xs_mt": 103, "kerma_mt": 403},
}
IMPLICIT_EXPONENT = re.compile(r"^(.+?)([+-]\d+)$")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def json_object(path: Path, label: str) -> tuple[bytes, dict[str, Any]]:
    raw = path.read_bytes()
    document = json.loads(raw)
    if not isinstance(document, dict):
        raise ValueError(f"{label} must be a JSON object")
    return raw, document


def safe_artifact(
    root: Path,
    relative_path: str,
    expected_sha256: str,
    expected_size: int | None = None,
) -> Path:
    candidate = Path(relative_path)
    if candidate.is_absolute() or ".." in candidate.parts:
        raise ValueError(f"artifact path is not normalized and relative: {relative_path!r}")
    resolved = (root / candidate).resolve(strict=True)
    try:
        resolved.relative_to(root)
    except ValueError as error:
        raise ValueError(f"artifact escapes its declared root: {relative_path!r}") from error
    if not resolved.is_file():
        raise ValueError(f"artifact is not a regular file: {relative_path!r}")
    observed_size = resolved.stat().st_size
    if expected_size is not None and observed_size != expected_size:
        raise ValueError(
            f"artifact size mismatch for {relative_path!r}: expected "
            f"{expected_size}, observed {observed_size}"
        )
    observed_sha256 = sha256_file(resolved)
    if observed_sha256 != expected_sha256:
        raise ValueError(
            f"artifact SHA-256 mismatch for {relative_path!r}: expected "
            f"{expected_sha256}, observed {observed_sha256}"
        )
    return resolved


def endf_float(field: str) -> float:
    value = field.strip()
    if not value:
        return 0.0
    try:
        return float(value)
    except ValueError:
        match = IMPLICIT_EXPONENT.fullmatch(value)
        if match is None:
            raise ValueError(f"invalid ENDF numeric field {field!r}") from None
        return float(f"{match.group(1)}e{match.group(2)}")


def endf_fields(line: str) -> list[str]:
    return [line[index : index + 11] for index in range(0, 66, 11)]


def endf_ids(line: str) -> tuple[int, int, int]:
    try:
        return int(line[66:70]), int(line[70:72]), int(line[72:75])
    except ValueError as error:
        raise ValueError("invalid ENDF MAT/MF/MT control fields") from error


def file3_tab1(path: Path, requested_mt: int) -> tuple[numpy.ndarray, numpy.ndarray]:
    lines = path.read_text(encoding="ascii").splitlines()
    starts = []
    previous_in_section = False
    for index, line in enumerate(lines):
        in_section = len(line) >= 75 and endf_ids(line)[1:] == (3, requested_mt)
        if in_section and not previous_in_section:
            starts.append(index)
        previous_in_section = in_section
    if len(starts) != 1:
        raise ValueError(
            f"{path} has {len(starts)} MF=3 MT={requested_mt} sections; expected one"
        )
    section = []
    for line in lines[starts[0] :]:
        if len(line) < 75:
            raise ValueError(f"short ENDF record in MF=3 MT={requested_mt}: {path}")
        _, mf, mt = endf_ids(line)
        if (mf, mt) != (3, requested_mt):
            break
        section.append(line)
    if len(section) < 3:
        raise ValueError(f"incomplete MF=3 MT={requested_mt} section in {path}")

    tab1 = endf_fields(section[1])
    try:
        interpolation_regions = int(tab1[4])
        point_count = int(tab1[5])
    except ValueError as error:
        raise ValueError(f"invalid MF=3 MT={requested_mt} TAB1 header in {path}") from error
    if interpolation_regions <= 0 or point_count < 2:
        raise ValueError(f"invalid MF=3 MT={requested_mt} TAB1 dimensions in {path}")
    interpolation_lines = (2 * interpolation_regions + 5) // 6
    data_fields = []
    for line in section[2 + interpolation_lines :]:
        data_fields.extend(endf_fields(line))
    if len(data_fields) < 2 * point_count:
        raise ValueError(f"truncated MF=3 MT={requested_mt} TAB1 data in {path}")
    values = [endf_float(field) for field in data_fields[: 2 * point_count]]
    energies = numpy.asarray(values[0::2], dtype=float)
    response = numpy.asarray(values[1::2], dtype=float)
    validate_curve(energies, response, f"MF=3 MT={requested_mt} in {path}")
    return energies, response


def validate_curve(energies: numpy.ndarray, values: numpy.ndarray, label: str) -> None:
    if (
        energies.ndim != 1
        or values.ndim != 1
        or len(energies) != len(values)
        or len(energies) < 2
        or not numpy.all(numpy.isfinite(energies))
        or not numpy.all(numpy.isfinite(values))
        or energies[0] < 0.0
        or not numpy.all(numpy.diff(energies) > 0.0)
    ):
        raise ValueError(f"invalid response curve: {label}")


def production_tape(run: dict[str, Any]) -> dict[str, Any]:
    matches = [
        entry["artifact"]
        for entry in run["output_tapes"]
        if entry["purpose"] == "production_heatr_pendf" and entry["unit"] == 23
    ]
    if len(matches) != 1:
        raise ValueError(
            f"NJOY run {run.get('nuclide')!r} has {len(matches)} production tape 23 artifacts"
        )
    return matches[0]


def tally_statistics(group: h5py.Group) -> dict[str, Any]:
    results = numpy.asarray(group["results"], dtype=float)
    if results.ndim != 3 or results.shape[2] != 2:
        raise ValueError(f"tally results have unexpected shape {results.shape}")
    realizations = int(group["n_realizations"][()])
    if realizations < 2:
        raise ValueError("tally has fewer than two realizations")
    summed = results[:, :, 0]
    summed_sq = results[:, :, 1]
    mean = summed / realizations
    variance = summed_sq / realizations - mean * mean
    variance = numpy.maximum(variance, 0.0)
    std = numpy.sqrt(variance / (realizations - 1))
    return {
        "mean": mean.reshape(-1),
        "std": std.reshape(-1),
        "n_realizations": realizations,
    }


def relative_difference(a: float, b: float) -> float:
    floor = max(abs(a), abs(b), 1.0e-300)
    return abs(a - b) / floor


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--statepoint", required=True, type=Path)
    parser.add_argument("--input-manifest", required=True, type=Path)
    parser.add_argument("--response-set", required=True, type=Path)
    parser.add_argument("--material", required=True, type=Path)
    parser.add_argument("--execution-profile", required=True, type=Path)
    parser.add_argument("--execution-root", required=True, type=Path)
    parser.add_argument("--execution-receipt", required=True, type=Path)
    parser.add_argument("--report-id", required=True)
    parser.add_argument("--output", required=True, type=Path)
    return parser.parse_args()


def main() -> None:
    arguments = parse_args()
    statepoint_path = arguments.statepoint.resolve(strict=True)
    manifest_path = arguments.input_manifest.resolve(strict=True)
    response_set_path = arguments.response_set.resolve(strict=True)
    material_path = arguments.material.resolve(strict=True)
    profile_path = arguments.execution_profile.resolve(strict=True)
    execution_root = arguments.execution_root.resolve(strict=True)
    receipt_path = arguments.execution_receipt.resolve(strict=True)
    inspector = Path(__file__).resolve(strict=True)

    manifest_raw, manifest = json_object(manifest_path, "OpenMC input manifest")
    response_raw, response_set = json_object(response_set_path, "neutron response set")
    material_raw, material = json_object(material_path, "material")
    profile_raw, profile = json_object(profile_path, "OpenMC execution profile")
    receipt_raw, receipt = json_object(receipt_path, "NJOY execution receipt")

    if manifest.get("schema_version") != INPUT_MANIFEST_SCHEMA:
        raise ValueError("unsupported OpenMC input manifest schema")
    if manifest.get("openmc_version") != TARGET_OPENMC_VERSION:
        raise ValueError("unsupported OpenMC version")
    if response_set.get("schema_version") != RESPONSE_SET_SCHEMA:
        raise ValueError("unsupported neutron response set schema")
    if not str(material.get("schema_version", "")).startswith(MATERIAL_SCHEMA_PREFIX):
        raise ValueError("unsupported material schema")
    if profile.get("schema_version") != EXECUTION_PROFILE_SCHEMA:
        raise ValueError("unsupported OpenMC execution profile schema")
    if profile.get("purpose") != "smoke_only":
        raise ValueError("execution profile is not the smoke profile")
    if receipt.get("schema_version") != EXECUTION_RECEIPT_SCHEMA:
        raise ValueError("unsupported NJOY execution receipt schema")
    if receipt.get("processor", {}).get("tool", {}).get("version") != TARGET_NJOY_VERSION:
        raise ValueError("unsupported NJOY version")

    bound = manifest.get("bindings", {})
    expected_bindings = {
        "response_set": (response_raw, "response_set"),
        "material": (material_raw, "material"),
        "execution_profile": (profile_raw, "execution_profile"),
    }
    for key, (raw, label) in expected_bindings.items():
        observed = bound.get(key, {}).get("sha256")
        expected = hashlib.sha256(raw).hexdigest()
        if observed != expected:
            raise ValueError(
                f"input manifest {label} binding {observed!r} differs from the "
                f"presented artifact {expected!r}"
            )

    root_receipt = execution_root / "openbnct-njoy-execution-receipt.json"
    if root_receipt.resolve(strict=True).read_bytes() != receipt_raw:
        raise ValueError("execution root receipt differs from the external trust anchor")

    density_g_cm3 = float(material["density_g_cm3"])
    mesh = manifest["scoring_mesh"]
    voxel_volume_cm3 = float(mesh["voxel_volume_cm3"])
    voxel_mass_g = float(mesh["voxel_mass_g"])
    voxel_mass_kg = voxel_mass_g * 1.0e-3
    expected_voxels = int(math.prod(int(d) for d in mesh["dimensions"]))

    with h5py.File(statepoint_path, "r") as statepoint:
        run_header = {
            "n_batches": int(statepoint["n_batches"][()]),
            "n_particles": int(statepoint["n_particles"][()]),
            "n_realizations": int(statepoint["n_realizations"][()]),
            "seed": int(statepoint["seed"][()]),
            "energy_mode": statepoint["energy_mode"][()].decode(),
        }
        if run_header["n_batches"] != int(profile["batches"]):
            raise ValueError("statepoint batch count differs from the execution profile")
        if run_header["seed"] != int(profile["seed"]):
            raise ValueError("statepoint seed differs from the execution profile")
        if run_header["energy_mode"] != profile["energy_mode"]:
            raise ValueError("statepoint energy mode differs from the execution profile")
        if run_header["n_particles"] != int(
            manifest["execution"]["particles_per_batch"]
        ):
            raise ValueError("statepoint batch size differs from the execution profile")

        tallies_root = statepoint["tallies"]
        filters_root = tallies_root["filters"]

        response_energy = numpy.asarray(response_set["energy_ev"], dtype=float)
        energyfunction_checks = []
        for component, filter_index in (("boron", 4), ("nitrogen", 5), ("hydrogen", 6)):
            stored = filters_root[f"filter {filter_index}"]
            executed_energy = numpy.asarray(stored["energy"], dtype=float)
            executed_y = numpy.asarray(stored["y"], dtype=float)
            sealed_y = numpy.asarray(response_set[COMPONENT_CURVES[component]], dtype=float)
            tables_identical = bool(
                numpy.array_equal(executed_energy, response_energy)
                and numpy.array_equal(executed_y, sealed_y)
            )
            energyfunction_checks.append(
                {
                    "component": component,
                    "filter_id": filter_index,
                    "knot_count": int(len(executed_energy)),
                    "executed_tables_bitwise_identical_to_response_set": tables_identical,
                }
            )
            if not tables_identical:
                raise ValueError(
                    f"executed energy-function table for {component} differs from "
                    f"the sealed response set"
                )

        tally_by_name: dict[str, dict[str, Any]] = {}
        for key in tallies_root.keys():
            if not key.startswith("tally "):
                continue
            group = tallies_root[key]
            name = group["name"][()]
            name = name.decode() if isinstance(name, bytes) else name
            stats = tally_statistics(group)
            tally_by_name[name] = {
                "id": int(key.split()[1]),
                "estimator": group["estimator"][()].decode(),
                "n_score_bins": int(group["n_score_bins"][()]),
                "filter_ids": [int(f) for f in group["filters"][()]],
                **stats,
            }

        manifest_names = [entry["name"] for entry in manifest["tallies"]]
        if sorted(tally_by_name) != sorted(manifest_names):
            raise ValueError(
                "statepoint tally set differs from the input manifest contract"
            )
        for entry in manifest["tallies"]:
            observed = tally_by_name[entry["name"]]
            if observed["id"] != int(entry["id"]):
                raise ValueError(f"tally id mismatch for {entry['name']!r}")
            if int(mesh["mesh_id"]) in observed["filter_ids"] and (
                len(observed["mean"]) % expected_voxels != 0
            ):
                raise ValueError(
                    f"tally {entry['name']!r} bin count {len(observed['mean'])} is "
                    f"not a multiple of the {expected_voxels} mesh voxels"
                )

        def totals(name: str) -> tuple[float, float]:
            tally = tally_by_name[name]
            return float(numpy.sum(tally["mean"])), float(
                numpy.sqrt(numpy.sum(tally["std"] ** 2))
            )

        # Closure: coupled heating vs neutron + photon heating (eV per source neutron).
        neutron_mean, neutron_std = totals(NEUTRON_HEATING_TALLY)
        photon_mean, photon_std = totals(PHOTON_HEATING_TALLY)
        coupled_mean, coupled_std = totals(COUPLED_HEATING_TALLY)
        closure_std = math.sqrt(neutron_std**2 + photon_std**2 + coupled_std**2)
        closure_difference = coupled_mean - (neutron_mean + photon_mean)
        closure_sigma = (
            abs(closure_difference) / closure_std if closure_std > 0.0 else 0.0
        )

        # Component response sum (Gy cm^3 per source neutron) vs neutron heating.
        component_total = numpy.zeros(expected_voxels, dtype=float)
        component_var = numpy.zeros(expected_voxels, dtype=float)
        component_summaries = {}
        for component, tally_name in COMPONENT_TALLIES.items():
            tally = tally_by_name[tally_name]
            if len(tally["mean"]) != expected_voxels:
                raise ValueError(
                    f"component tally {tally_name!r} does not cover the scoring mesh"
                )
            component_total += tally["mean"]
            component_var += tally["std"] ** 2
            component_summaries[component] = {
                "total_gy_cm3_per_source_neutron": float(numpy.sum(tally["mean"])),
                "total_uncertainty_1sigma": float(
                    numpy.sqrt(numpy.sum(tally["std"] ** 2))
                ),
            }
        component_total_std = float(numpy.sqrt(numpy.sum(component_var)))
        # Gy cm^3 * (g/cm^3) = Gy g = 1e-3 J; convert to eV.
        component_total_ev = (
            float(numpy.sum(component_total)) * density_g_cm3 * 1.0e-3 * EV_PER_JOULE
        )
        component_vs_heating = {
            "component_sum_ev_per_source_neutron": component_total_ev,
            "component_sum_uncertainty_1sigma_ev": (
                component_total_std * density_g_cm3 * 1.0e-3 * EV_PER_JOULE
            ),
            "neutron_heating_ev_per_source_neutron": neutron_mean,
            "neutron_heating_uncertainty_1sigma_ev": neutron_std,
            "relative_difference": relative_difference(component_total_ev, neutron_mean),
        }

        # Reaction-rate times independently derived mean deposited energy per reaction.
        neutron_fluence = tally_by_name[NEUTRON_FLUENCE_TALLY]
        energy_bins = numpy.asarray(
            filters_root["filter 7"]["bins"], dtype=float
        )
        n_energy_bins = int(len(energy_bins) - 1)
        # OpenMC results flatten filter bins with the last filter innermost:
        # [mesh voxel][energy bin] for the neutron fluence diagnostic.
        fluence_per_bin = numpy.sum(
            neutron_fluence["mean"].reshape(expected_voxels, n_energy_bins), axis=0
        )
        fluence_unc_per_bin = numpy.sqrt(
            numpy.sum(
                neutron_fluence["std"].reshape(expected_voxels, n_energy_bins) ** 2,
                axis=0,
            )
        )

        run_by_nuclide = {entry["nuclide"]: entry for entry in receipt["runs"]}
        reaction_audits = {}
        for component, spec in AUDIT_REACTIONS.items():
            rate_mean, rate_std = totals(spec["tally"])
            if rate_mean <= 0.0:
                raise ValueError(f"audit tally {spec['tally']!r} has no reactions")
            response_mean, response_std = totals(COMPONENT_TALLIES[component])
            measured_joules = response_mean * density_g_cm3 * 1.0e-3
            measured_ev = measured_joules * EV_PER_JOULE / rate_mean
            measured_std_ev = (
                measured_ev
                * math.sqrt(
                    (response_std / max(response_mean, 1.0e-300)) ** 2
                    + (rate_std / rate_mean) ** 2
                )
            )

            run = run_by_nuclide[spec["nuclide"]]
            tape = production_tape(run)
            tape_path = safe_artifact(
                execution_root, tape["path"], tape["sha256"], tape["size_bytes"]
            )
            xs_energy, xs = file3_tab1(tape_path, spec["xs_mt"])
            kerma_energy, kerma = file3_tab1(tape_path, spec["kerma_mt"])
            # PENDF MT 4xx KERMA factors are stored in eV barns, so the ratio
            # KERMA/xs is the evaluated mean deposited energy per reaction in eV.
            kerma_on_xs = numpy.interp(kerma_energy, xs_energy, xs)
            mask = kerma_on_xs > 0.0
            if not numpy.any(mask):
                raise ValueError(
                    f"{spec['nuclide']} MT {spec['xs_mt']} has no positive cross section"
                )
            e_dep_ev = kerma[mask] / kerma_on_xs[mask]

            # Fold the evaluated mean deposited energy over the measured
            # spectrum: <E_dep> = sum_b phi_b * int_b sigma*E_dep / int_b sigma.
            folded_num = 0.0
            folded_den = 0.0
            e_dep_interp = numpy.zeros_like(xs)
            valid = xs > 0.0
            e_dep_interp[valid] = (
                numpy.interp(xs_energy[valid], kerma_energy, kerma) / xs[valid]
            )
            for b in range(n_energy_bins):
                lo, hi = energy_bins[b], energy_bins[b + 1]
                sel = valid & (xs_energy >= lo) & (xs_energy < hi)
                if not numpy.any(sel):
                    continue
                num = numpy.trapezoid(xs[sel] * e_dep_interp[sel], xs_energy[sel])
                den = numpy.trapezoid(xs[sel], xs_energy[sel])
                if den > 0.0:
                    folded_num += fluence_per_bin[b] * num / den
                    folded_den += fluence_per_bin[b]
            expected_ev = folded_num / folded_den if folded_den > 0.0 else float("nan")

            reaction_audits[component] = {
                "tally": spec["tally"],
                "nuclide": spec["nuclide"],
                "reaction_mt": spec["xs_mt"],
                "partial_kerma_mt": spec["kerma_mt"],
                "reactions_per_source_neutron": rate_mean,
                "reaction_uncertainty_1sigma": rate_std,
                "measured_mean_deposited_energy_ev_per_reaction": measured_ev,
                "measured_uncertainty_1sigma_ev": measured_std_ev,
                "evaluated_mean_deposited_energy_ev_per_reaction": float(expected_ev),
                "evaluated_energy_domain_ev": [
                    float(kerma_energy[mask][0]),
                    float(kerma_energy[mask][-1]),
                ],
                "evaluated_mean_deposited_energy_range_ev": [
                    float(numpy.min(e_dep_ev)),
                    float(numpy.max(e_dep_ev)),
                ],
                "measured_over_evaluated": measured_ev / expected_ev
                if math.isfinite(expected_ev) and expected_ev > 0.0
                else None,
            }

        tally_summaries = {}
        for entry in manifest["tallies"]:
            name = entry["name"]
            tally = tally_by_name[name]
            total, total_std = totals(name)
            tally_summaries[name] = {
                "estimator": tally["estimator"],
                "n_score_bins": tally["n_score_bins"],
                "n_realizations": tally["n_realizations"],
                "raw_unit": entry["raw_unit"],
                "total": total,
                "total_uncertainty_1sigma": total_std,
                "nonzero_bins": int(numpy.count_nonzero(tally["mean"])),
            }

        report = {
            "schema_version": REPORT_SCHEMA,
            "id": arguments.report_id,
            "case_id": manifest["case_id"],
            "inspection": {
                "method": COMPARISON_METHOD,
                "source_sha256": sha256_file(inspector),
                "python_version": platform.python_version(),
                "numpy_version": numpy.__version__,
                "h5py_version": h5py.__version__,
                "hdf5_library_version": h5py.version.hdf5_version,
            },
            "bindings": {
                "statepoint": {
                    "path": statepoint_path.name,
                    "sha256": sha256_file(statepoint_path),
                    "size_bytes": statepoint_path.stat().st_size,
                },
                "openmc_input_manifest": {
                    "case_id": manifest["case_id"],
                    "backend_id": manifest["backend_id"],
                    "sha256": hashlib.sha256(manifest_raw).hexdigest(),
                },
                "neutron_response_set": {
                    "id": response_set["id"],
                    "sha256": hashlib.sha256(response_raw).hexdigest(),
                },
                "material": {
                    "id": material["id"],
                    "sha256": hashlib.sha256(material_raw).hexdigest(),
                },
                "openmc_execution_profile": {
                    "id": profile["id"],
                    "sha256": hashlib.sha256(profile_raw).hexdigest(),
                },
                "njoy_execution_receipt": {
                    "id": receipt["id"],
                    "sha256": hashlib.sha256(receipt_raw).hexdigest(),
                },
            },
            "run_header": run_header,
            "executed_response_table_checks": energyfunction_checks,
            "tally_contract": {
                "manifest_tally_count": len(manifest_names),
                "statepoint_tally_count": len(tally_by_name),
                "all_contract_tallies_present": True,
            },
            "tally_summaries": tally_summaries,
            "estimator_comparisons": {
                "coupled_heating_closure": {
                    "coupled_ev_per_source_neutron": coupled_mean,
                    "neutron_plus_photon_ev_per_source_neutron": (
                        neutron_mean + photon_mean
                    ),
                    "difference_ev_per_source_neutron": closure_difference,
                    "combined_uncertainty_1sigma_ev": closure_std,
                    "difference_in_combined_sigma": closure_sigma,
                },
                "component_sum_vs_neutron_heating": component_vs_heating,
                "reaction_rate_times_mean_deposited_energy": reaction_audits,
                "component_response_totals": component_summaries,
                "measured_spectrum_energy_bins_ev": energy_bins.tolist(),
                "measured_spectrum_fluence_per_bin": fluence_per_bin.tolist(),
                "measured_spectrum_uncertainty_per_bin": fluence_unc_per_bin.tolist(),
            },
            "correlation_disclaimer": (
                "All tallies share transport histories; estimator comparisons are "
                "correlated diagnostics, not independent validation (ADR 0005)."
            ),
            "qualification": "smoke_execution_evidence_not_dose_qualification",
        }

    with arguments.output.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(report, stream, indent=2, allow_nan=False)
        stream.write("\n")


if __name__ == "__main__":
    try:
        main()
    except (KeyError, OSError, ValueError) as error:
        raise SystemExit(f"error: {error}") from error

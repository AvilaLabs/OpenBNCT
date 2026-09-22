#!/usr/bin/env python3
# SPDX-License-Identifier: MIT

"""Compare controlled NJOY MT 301 PENDF data with inspected OpenMC HDF5 data."""

from __future__ import annotations

import argparse
import hashlib
import json
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


REPORT_SCHEMA = "openbnct.openmc-njoy-heating-comparison/0.1.0"
COMPARISON_METHOD = "openbnct-openmc-njoy-heating-comparator/0.1.0"
MANIFEST_SCHEMA = "openbnct.openmc-nuclear-data-manifest/0.3.0"
EXECUTION_RECEIPT_SCHEMA = "openbnct.njoy-execution-receipt/0.1.0"
TARGET_OPENMC_VERSION = "0.16.0"
TARGET_NJOY_VERSION = "2016.78"
TARGET_RESPONSE_MT = 301
LOCAL_RESPONSE_MT = 901
TARGET_TEMPERATURE_K = 293.6
TEMPERATURE_TOLERANCE_K = 0.5
RELATIVE_TOLERANCE = 1.0e-6
RELATIVE_FLOOR_FRACTION = 1.0e-12
ENERGY_GRID_RELATIVE_TOLERANCE = 1.0e-6
K_BOLTZMANN_EV_PER_K = 8.617333262e-5
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


def selected_temperature_label(group: h5py.Group, path: Path) -> str:
    matches = []
    for label, dataset in group["kTs"].items():
        temperature = float(dataset[()]) / K_BOLTZMANN_EV_PER_K
        if abs(temperature - TARGET_TEMPERATURE_K) <= TEMPERATURE_TOLERANCE_K:
            matches.append(label)
    if len(matches) != 1:
        raise ValueError(
            f"{path} has {len(matches)} tables within {TEMPERATURE_TOLERANCE_K} K "
            f"of {TARGET_TEMPERATURE_K} K"
        )
    return matches[0]


def hdf5_reaction(
    path: Path, nuclide: str, mt: int
) -> tuple[numpy.ndarray, numpy.ndarray]:
    with h5py.File(path, "r") as handle:
        if handle.attrs.get("filetype", b"") not in (b"data_neutron", "data_neutron"):
            raise ValueError(f"not an OpenMC incident-neutron HDF5 file: {path}")
        root = handle.get(nuclide)
        if not isinstance(root, h5py.Group):
            raise ValueError(f"{path} lacks root group {nuclide!r}")
        label = selected_temperature_label(root, path)
        grid = numpy.asarray(root[f"energy/{label}"], dtype=float)
        reaction = root.get(f"reactions/reaction_{mt:03}")
        if not isinstance(reaction, h5py.Group) or int(reaction.attrs.get("mt", -1)) != mt:
            raise ValueError(f"{path} lacks reaction MT {mt}")
        dataset = reaction.get(f"{label}/xs")
        if not isinstance(dataset, h5py.Dataset):
            raise ValueError(f"{path} lacks MT {mt} response at {label}")
        threshold = int(dataset.attrs.get("threshold_idx", -1))
        values = numpy.asarray(dataset, dtype=float)
        if threshold < 0 or threshold + len(values) > len(grid):
            raise ValueError(f"{path} has invalid MT {mt} threshold metadata")
        energies = grid[threshold : threshold + len(values)]
    validate_curve(energies, values, f"HDF5 MT {mt} in {path}")
    return energies, values


def response_difference_summary(
    reference: numpy.ndarray, candidate: numpy.ndarray, energies: numpy.ndarray
) -> dict[str, float]:
    absolute = numpy.abs(reference - candidate)
    floor = max(float(numpy.max(numpy.abs(reference))), float(numpy.max(numpy.abs(candidate))))
    floor *= RELATIVE_FLOOR_FRACTION
    relative = absolute / numpy.maximum(
        numpy.maximum(numpy.abs(reference), numpy.abs(candidate)), floor
    )
    relative_index = int(numpy.argmax(relative))
    absolute_index = int(numpy.argmax(absolute))
    return {
        "maximum_relative_difference": float(relative[relative_index]),
        "maximum_relative_difference_energy_ev": float(energies[relative_index]),
        "maximum_absolute_difference_ev_barn": float(absolute[absolute_index]),
        "maximum_absolute_difference_energy_ev": float(energies[absolute_index]),
    }


def energy_grid_difference_summary(
    reference: numpy.ndarray, candidate: numpy.ndarray
) -> dict[str, float]:
    absolute = numpy.abs(reference - candidate)
    relative = absolute / numpy.maximum(
        numpy.maximum(numpy.abs(reference), numpy.abs(candidate)),
        numpy.finfo(float).tiny,
    )
    relative_index = int(numpy.argmax(relative))
    absolute_index = int(numpy.argmax(absolute))
    return {
        "maximum_relative_difference": float(relative[relative_index]),
        "maximum_relative_difference_at_reference_energy_ev": float(
            reference[relative_index]
        ),
        "maximum_absolute_difference_ev": float(absolute[absolute_index]),
        "maximum_absolute_difference_at_reference_energy_ev": float(
            reference[absolute_index]
        ),
    }


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


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-root", required=True, type=Path)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--execution-root", required=True, type=Path)
    parser.add_argument("--execution-receipt", required=True, type=Path)
    parser.add_argument("--report-id", required=True)
    parser.add_argument("--output", required=True, type=Path)
    return parser.parse_args()


def main() -> None:
    arguments = parse_args()
    data_root = arguments.data_root.resolve(strict=True)
    execution_root = arguments.execution_root.resolve(strict=True)
    manifest_path = arguments.manifest.resolve(strict=True)
    receipt_path = arguments.execution_receipt.resolve(strict=True)
    inspector = Path(__file__).resolve(strict=True)
    manifest_raw, manifest = json_object(manifest_path, "OpenMC manifest")
    receipt_raw, receipt = json_object(receipt_path, "NJOY execution receipt")

    if manifest.get("schema_version") != MANIFEST_SCHEMA:
        raise ValueError("unsupported OpenMC manifest schema")
    if manifest.get("openmc_version") != TARGET_OPENMC_VERSION:
        raise ValueError("unsupported OpenMC version")
    if receipt.get("schema_version") != EXECUTION_RECEIPT_SCHEMA:
        raise ValueError("unsupported NJOY execution receipt schema")
    if receipt.get("processor", {}).get("tool", {}).get("version") != TARGET_NJOY_VERSION:
        raise ValueError("unsupported NJOY version")
    if receipt.get("case_id") != "nf-bnct-001":
        raise ValueError("comparison requires the NF-BNCT-001 execution receipt")

    root_receipt = execution_root / "openbnct-njoy-execution-receipt.json"
    if root_receipt.resolve(strict=True).read_bytes() != receipt_raw:
        raise ValueError("execution root receipt differs from the external trust anchor")

    neutron_tables = manifest.get("neutron_tables")
    runs = receipt.get("runs")
    if not isinstance(neutron_tables, list) or not isinstance(runs, list):
        raise ValueError("manifest neutron tables and execution receipt runs must be arrays")
    table_by_nuclide = {entry["nuclide"]: entry for entry in neutron_tables}
    run_by_nuclide = {entry["nuclide"]: entry for entry in runs}
    if len(table_by_nuclide) != len(neutron_tables) or len(run_by_nuclide) != len(runs):
        raise ValueError("duplicate nuclide in compared evidence")
    if set(table_by_nuclide) != set(run_by_nuclide):
        raise ValueError("OpenMC manifest and NJOY receipt nuclide sets differ")

    results = []
    for nuclide in sorted(table_by_nuclide):
        table = table_by_nuclide[nuclide]
        run = run_by_nuclide[nuclide]
        if run.get("exit_code") != 0 or not run.get("production_diagnostic_pendf_identical"):
            raise ValueError(f"NJOY run {nuclide} lacks a completed identical production PENDF")
        openmc_artifact = table["artifact"]
        openmc_path = safe_artifact(
            data_root,
            openmc_artifact["relative_path"],
            openmc_artifact["sha256"],
        )
        njoy_artifact = production_tape(run)
        njoy_path = safe_artifact(
            execution_root,
            njoy_artifact["path"],
            njoy_artifact["sha256"],
            njoy_artifact["size_bytes"],
        )

        njoy_energy, njoy_heating = file3_tab1(njoy_path, TARGET_RESPONSE_MT)
        openmc_energy, openmc_heating = hdf5_reaction(
            openmc_path, nuclide, TARGET_RESPONSE_MT
        )
        if len(openmc_energy) != len(njoy_energy):
            raise ValueError(f"MT 301 energy-grid point counts differ for {nuclide}")
        grid_comparison = energy_grid_difference_summary(openmc_energy, njoy_energy)
        grid_correspondence = (
            grid_comparison["maximum_relative_difference"]
            <= ENERGY_GRID_RELATIVE_TOLERANCE
        )
        if not grid_correspondence:
            raise ValueError(f"MT 301 energy grids do not correspond for {nuclide}")
        comparison = response_difference_summary(
            openmc_heating, njoy_heating, openmc_energy
        )
        within_tolerance = (
            comparison["maximum_relative_difference"] <= RELATIVE_TOLERANCE
        )

        local_energy, local_heating = hdf5_reaction(
            openmc_path, nuclide, LOCAL_RESPONSE_MT
        )
        if len(openmc_energy) != len(local_energy):
            raise ValueError(
                f"OpenMC MT 301 and 901 energy-grid point counts differ for {nuclide}"
            )
        local_grid_comparison = energy_grid_difference_summary(
            openmc_energy, local_energy
        )
        if (
            local_grid_comparison["maximum_relative_difference"]
            > ENERGY_GRID_RELATIVE_TOLERANCE
        ):
            raise ValueError(
                f"OpenMC MT 301 and 901 energy grids do not correspond for {nuclide}"
            )
        local_comparison = response_difference_summary(
            openmc_heating, local_heating, openmc_energy
        )
        local_equivalent = (
            local_comparison["maximum_relative_difference"] <= RELATIVE_TOLERANCE
        )
        photon_production_mts = table["photon_production_mts"]
        effective_local_fallback = not photon_production_mts and local_equivalent

        results.append(
            {
                "nuclide": nuclide,
                "openmc_hdf5": {
                    "relative_path": openmc_artifact["relative_path"],
                    "size_bytes": openmc_path.stat().st_size,
                    "sha256": openmc_artifact["sha256"],
                },
                "njoy_production_pendf": njoy_artifact,
                "point_count": len(openmc_energy),
                "energy_range_ev": [float(openmc_energy[0]), float(openmc_energy[-1])],
                "energy_grid_bitwise_identical": bool(
                    numpy.array_equal(openmc_energy, njoy_energy)
                ),
                "energy_grid_correspondence": {
                    **grid_comparison,
                    "within_relative_tolerance": grid_correspondence,
                },
                **comparison,
                "within_relative_tolerance": within_tolerance,
                "openmc_local_heating_comparison": {
                    "response_mt": LOCAL_RESPONSE_MT,
                    **local_comparison,
                    "equivalent_within_relative_tolerance": local_equivalent,
                },
                "photon_production_mts": photon_production_mts,
                "effective_local_photon_fallback": effective_local_fallback,
            }
        )

    report = {
        "schema_version": REPORT_SCHEMA,
        "id": arguments.report_id,
        "case_id": receipt["case_id"],
        "inspection": {
            "method": COMPARISON_METHOD,
            "source_sha256": sha256_file(inspector),
            "python_version": platform.python_version(),
            "numpy_version": numpy.__version__,
            "h5py_version": h5py.__version__,
            "hdf5_library_version": h5py.version.hdf5_version,
        },
        "bindings": {
            "openmc_nuclear_data_manifest": {
                "id": manifest["id"],
                "sha256": hashlib.sha256(manifest_raw).hexdigest(),
            },
            "njoy_execution_receipt": {
                "id": receipt["id"],
                "sha256": hashlib.sha256(receipt_raw).hexdigest(),
            },
        },
        "comparison": {
            "response_mt": TARGET_RESPONSE_MT,
            "temperature_k": TARGET_TEMPERATURE_K,
            "energy_grid_requirement": "pointwise_corresponding_without_interpolation",
            "energy_grid_relative_difference_tolerance": ENERGY_GRID_RELATIVE_TOLERANCE,
            "interpolation": "none",
            "relative_difference_tolerance": RELATIVE_TOLERANCE,
            "relative_difference_floor_fraction": RELATIVE_FLOOR_FRACTION,
        },
        "results": results,
        "summary": {
            "nuclide_count": len(results),
            "all_energy_grids_correspond": all(
                result["energy_grid_correspondence"]["within_relative_tolerance"]
                for result in results
            ),
            "all_mt301_within_relative_tolerance": all(
                result["within_relative_tolerance"] for result in results
            ),
            "maximum_mt301_relative_difference": max(
                result["maximum_relative_difference"] for result in results
            ),
            "effective_local_photon_fallback_nuclides": [
                result["nuclide"]
                for result in results
                if result["effective_local_photon_fallback"]
            ],
        },
        "qualification": "comparison_only_not_response_qualification",
    }
    with arguments.output.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(report, stream, indent=2, allow_nan=False)
        stream.write("\n")


if __name__ == "__main__":
    try:
        main()
    except (KeyError, OSError, ValueError) as error:
        raise SystemExit(f"error: {error}") from error

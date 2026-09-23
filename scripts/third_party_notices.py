#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Generate a third-party license/notice inventory from Cargo.lock.

Walks the `cargo metadata` resolve graph from a set of root packages over
normal (non-dev, non-build) dependency edges and emits a single UTF-8 text
file listing every third-party package's name, version, declared license,
repository, and the full text of any license/notice files found in its
published source. This is what ships as THIRD_PARTY_NOTICES.txt in binary,
wasm, and wheel releases: it is generated from Cargo.lock rather than
hand-maintained so it cannot silently drift the way a hand-written table
does.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path
from typing import Iterable

# Case-insensitive filename prefixes that mark a license/notice file at the
# root of a package's source directory.
LICENSE_FILENAME_PREFIXES = (
    "license",
    "licence",
    "copying",
    "notice",
    "copyright",
    "unlicense",
)


def run_cargo_metadata(manifest_path: str | None) -> dict:
    """Invoke `cargo metadata` and return the parsed document.

    Always `--locked --offline`: the report must reflect exactly the
    committed Cargo.lock, and generation must never touch the network.
    """
    command = ["cargo", "metadata", "--format-version", "1", "--locked", "--offline"]
    if manifest_path is not None:
        command += ["--manifest-path", manifest_path]
    try:
        completed = subprocess.run(command, capture_output=True, text=True, check=False)
    except FileNotFoundError as error:
        raise SystemExit(f"error: cargo not found on PATH ({error})") from error
    if completed.returncode != 0:
        raise SystemExit(
            "error: cargo metadata failed (exit "
            f"{completed.returncode}):\n{completed.stderr.strip()}"
        )
    return json.loads(completed.stdout)


def resolve_root_ids(metadata: dict, root_names: Iterable[str]) -> list[str]:
    """Map root package names to their cargo-metadata package ids.

    Prefers a workspace member with the given name, then any other
    path-source (local, unpublished) package with that name. Raises if a
    name matches nothing, since a typo'd --root should fail loudly rather
    than silently produce an incomplete report.
    """
    packages_by_id = {package["id"]: package for package in metadata["packages"]}
    workspace_members = set(metadata["workspace_members"])

    by_name: dict[str, list[str]] = {}
    for package_id, package in packages_by_id.items():
        by_name.setdefault(package["name"], []).append(package_id)

    root_ids = []
    for name in root_names:
        candidates = by_name.get(name, [])
        if not candidates:
            raise SystemExit(f"error: --root {name!r} matches no package in cargo metadata output")
        workspace_candidates = [c for c in candidates if c in workspace_members]
        local_candidates = [c for c in candidates if packages_by_id[c].get("source") is None]
        chosen = workspace_candidates or local_candidates or candidates
        # Sorted for determinism in the pathological case of a genuine
        # name collision across sources.
        root_ids.append(sorted(chosen)[0])
    return root_ids


def normal_dep_ids(node: dict) -> set[str]:
    """Package ids reachable from a resolve node over a normal dependency edge.

    A `dep_kinds` entry with `kind: null` is a normal (non-dev, non-build)
    edge; dev- and build-only edges are excluded so the notices only cover
    code that actually ships.
    """
    ids = set()
    for dep in node.get("deps", []):
        if any(kind.get("kind") is None for kind in dep.get("dep_kinds", [])):
            ids.add(dep["pkg"])
    return ids


def walk_normal_graph(metadata: dict, root_ids: Iterable[str]) -> set[str]:
    """All package ids reachable from the roots over normal edges only.

    Traversal passes through local path/workspace packages — their own
    normal deps still count — but never follows a dev- or build-only edge
    anywhere in the graph.
    """
    nodes_by_id = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    seen: set[str] = set()
    frontier = list(root_ids)
    while frontier:
        package_id = frontier.pop()
        if package_id in seen:
            continue
        seen.add(package_id)
        node = nodes_by_id.get(package_id)
        if node is None:
            continue
        for dep_id in normal_dep_ids(node):
            if dep_id not in seen:
                frontier.append(dep_id)
    return seen


def third_party_packages(metadata: dict, root_ids: Iterable[str]) -> list[dict]:
    """Third-party (non-path, non-workspace) packages reachable from the roots.

    A null `source` marks a workspace member or other local path
    dependency; those are traversed through but never listed themselves.
    Sorted by name then version for deterministic output.
    """
    packages_by_id = {package["id"]: package for package in metadata["packages"]}
    reachable = walk_normal_graph(metadata, root_ids)
    packages = [
        packages_by_id[package_id]
        for package_id in reachable
        if packages_by_id[package_id].get("source") is not None
    ]
    packages.sort(key=lambda package: (package["name"], package["version"]))
    return packages


def is_license_filename(filename: str) -> bool:
    lowered = filename.lower()
    return any(lowered.startswith(prefix) for prefix in LICENSE_FILENAME_PREFIXES)


# Font-license texts live under a fonts/ subdirectory in some crates rather
# than at the package root. egui's bundled epaint_default_fonts ships one .txt
# per font there (OFL.txt, UFL.txt, Hack-Regular.txt, ...) and nothing else in
# .txt form, so every .txt under fonts/ is treated as a license text.
def is_fonts_license_filename(filename: str) -> bool:
    return filename.lower().endswith(".txt")


def discover_license_files(source_dir: Path) -> list[Path]:
    """License/notice files for one package's source directory.

    Matches LICENSE*/LICENCE*/COPYING*/NOTICE*/COPYRIGHT*/UNLICENSE* at the
    package root (case-insensitive), plus every *.txt file under a fonts/
    subdirectory — the layout epaint_default_fonts uses for its bundled font
    license texts (Hack-Regular.txt is the Hack license, for example).
    """
    if not source_dir.is_dir():
        return []
    found = [entry for entry in source_dir.iterdir() if entry.is_file() and is_license_filename(entry.name)]
    fonts_dir = source_dir / "fonts"
    if fonts_dir.is_dir():
        found += [entry for entry in fonts_dir.iterdir() if entry.is_file() and is_fonts_license_filename(entry.name)]
    found.sort(key=lambda path: path.relative_to(source_dir).as_posix())
    return found


def sha256_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


class LicenseTextPool:
    """Deduplicates license texts by content hash and assigns stable ids.

    Ids are assigned in first-seen order, so a caller that always visits
    packages and files in a fixed (sorted) order gets fully deterministic
    ids across runs.
    """

    def __init__(self) -> None:
        self._id_by_hash: dict[str, str] = {}
        self.entries: list[tuple[str, str, str]] = []  # (id, sha256, text)

    def add(self, text: str) -> str:
        digest = sha256_text(text)
        existing = self._id_by_hash.get(digest)
        if existing is not None:
            return existing
        new_id = f"L{len(self.entries) + 1}"
        self._id_by_hash[digest] = new_id
        self.entries.append((new_id, digest, text))
        return new_id


def describe_license(package: dict) -> str:
    license_expression = package.get("license")
    if license_expression:
        return license_expression
    license_file = package.get("license_file")
    if license_file:
        return f"see {license_file}"
    return "UNSPECIFIED"


def render_report(metadata: dict, root_names: list[str], extras: list[tuple[str, Path]]) -> str:
    root_ids = resolve_root_ids(metadata, root_names)
    packages = third_party_packages(metadata, root_ids)
    pool = LicenseTextPool()

    package_blocks = []
    for package in packages:
        source_dir = Path(package["manifest_path"]).parent
        license_files = discover_license_files(source_dir)
        license_expression = describe_license(package)
        repository = package.get("repository") or "not declared"
        lines = [
            f"{package['name']} {package['version']}",
            f"  License: {license_expression}",
            f"  Repository: {repository}",
        ]
        if license_files:
            lines.append("  License files:")
            for path in license_files:
                text = path.read_text(encoding="utf-8", errors="replace")
                text_id = pool.add(text)
                lines.append(f"    {path.relative_to(source_dir).as_posix()} -> [{text_id}]")
        else:
            lines.append(
                "  License text: not bundled with this package — the standard"
                f" text of {license_expression} applies."
            )
        package_blocks.append("\n".join(lines))

    extra_blocks = []
    for name, path in extras:
        text = path.read_text(encoding="utf-8", errors="replace")
        text_id = pool.add(text)
        extra_blocks.append(f"{name}\n  License text: [{text_id}]")

    sections = [
        "Third-Party Notices for OpenBNCT\n"
        "Generated by scripts/third_party_notices.py from Cargo.lock — do not\n"
        "edit by hand; regenerate instead.\n\n"
        "Lists every third-party package reachable from "
        f"{', '.join(root_names)} over normal (non-dev,\n"
        "non-build) dependency edges, together with the license text found in\n"
        "its published source."
    ]
    sections.append("=" * 80 + "\nPACKAGES\n" + "=" * 80 + "\n\n" + "\n\n".join(package_blocks))
    if extra_blocks:
        sections.append("=" * 80 + "\nADDITIONAL COMPONENTS\n" + "=" * 80 + "\n\n" + "\n\n".join(extra_blocks))
    text_sections = [f"[{text_id}] sha256:{digest}\n" + "-" * 80 + f"\n{text}" for text_id, digest, text in pool.entries]
    sections.append("=" * 80 + "\nLICENSE TEXTS\n" + "=" * 80 + "\n\n" + "\n\n".join(text_sections))

    return "\n\n".join(sections) + "\n"


def parse_extra(value: str) -> tuple[str, Path]:
    name, separator, file_path = value.partition("=")
    if not separator or not name or not file_path:
        raise argparse.ArgumentTypeError(f"--extra must be NAME=FILE, got {value!r}")
    return name, Path(file_path)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path, help="path to write the notices text file")
    parser.add_argument(
        "--root",
        action="append",
        dest="roots",
        default=None,
        metavar="PKG",
        help="root package name to walk from (repeatable; default: openbnct-cli, openbnct-gui,"
        " or the manifest's own root package under --manifest-path)",
    )
    parser.add_argument("--manifest-path", default=None, help="cargo manifest to run `cargo metadata` against")
    parser.add_argument(
        "--extra",
        action="append",
        dest="extras",
        default=[],
        type=parse_extra,
        metavar="NAME=FILE",
        help="append an extra component with its own license file (repeatable)",
    )
    arguments = parser.parse_args()

    for name, path in arguments.extras:
        if not path.is_file():
            raise SystemExit(f"error: --extra {name!r} license file not found: {path}")

    metadata = run_cargo_metadata(arguments.manifest_path)

    if arguments.roots:
        root_names = arguments.roots
    else:
        default_root_id = metadata["resolve"].get("root")
        if default_root_id is not None:
            packages_by_id = {package["id"]: package for package in metadata["packages"]}
            root_names = [packages_by_id[default_root_id]["name"]]
        else:
            root_names = ["openbnct-cli", "openbnct-gui"]

    # Written as bytes so the output is identical on every platform (text
    # mode would translate newlines to CRLF on Windows).
    encoded = render_report(metadata, root_names, arguments.extras).encode("utf-8")
    arguments.output.write_bytes(encoded)
    print(f"wrote {arguments.output} ({len(encoded)} bytes, roots: {', '.join(root_names)})", file=sys.stderr)


if __name__ == "__main__":
    main()

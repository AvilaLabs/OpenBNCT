#!/usr/bin/env python3
"""Require the complete openbnct release set: one sdist + one wheel per platform."""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import sys
import tomllib


# macOS wheel tags carry a deployment target (e.g. macosx_12_0_arm64), so those
# entries are matched as (prefix, arch) rather than a literal substring.
EXPECTED_PLATFORMS = (
    ("manylinux_2_17_x86_64", None),
    ("manylinux_2_17_aarch64", None),
    ("macosx", "x86_64"),
    ("macosx", "arm64"),
    ("win_amd64", None),
)


def platform_present(wheel: str, tag: str, arch: str | None) -> bool:
    if arch is None:
        return tag in wheel
    # The platform tag is the final dotted component: e.g. `macosx_12_0_arm64.whl`.
    return tag in wheel and wheel.rsplit(".", 1)[0].endswith(arch)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dist", type=Path, help="directory of collected distributions")
    arguments = parser.parse_args()

    root = Path(__file__).resolve().parents[1]
    version = tomllib.loads(
        (root / "bindings" / "python" / "pyproject.toml").read_text(encoding="utf-8")
    )["project"]["version"]

    files = {path.name for path in arguments.dist.iterdir() if path.is_file()}
    errors: list[str] = []

    sdist = f"openbnct-{version}.tar.gz"
    if sdist not in files:
        errors.append(f"missing sdist {sdist}")

    wheel_pattern = re.compile(rf"openbnct-{re.escape(version)}-.+\.whl$")
    wheels = [name for name in files if wheel_pattern.fullmatch(name)]
    for tag, arch in EXPECTED_PLATFORMS:
        if not any(platform_present(wheel, tag, arch) for wheel in wheels):
            expected = tag if arch is None else f"{tag}_…_{arch}"
            errors.append(f"missing wheel for platform tag containing {expected!r}")
    foreign = [
        name
        for name in files
        if name.endswith(".whl") and not wheel_pattern.fullmatch(name)
    ]
    if foreign:
        errors.append(f"wheels not matching version {version}: {sorted(foreign)}")

    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
    print(f"release set OK: {len(wheels)} wheels + sdist for openbnct {version}")


if __name__ == "__main__":
    main()

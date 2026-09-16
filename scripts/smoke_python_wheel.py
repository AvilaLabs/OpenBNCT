#!/usr/bin/env python3
"""Install one openbnct wheel in an isolated environment and exercise it."""

from __future__ import annotations

import argparse
from pathlib import Path
import subprocess
import sys
import tempfile
import venv


def command(arguments: list[str | Path]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(argument) for argument in arguments],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dist", type=Path, help="directory containing built wheels")
    arguments = parser.parse_args()

    suffix = "win_amd64" if sys.platform == "win32" else (
        "macosx_arm64" if sys.platform == "darwin" and "arm" in __import__("platform").machine().lower()
        else "macosx_x86_64" if sys.platform == "darwin"
        else "manylinux" if sys.platform == "linux" and "aarch" in __import__("platform").machine().lower()
        else "manylinux"
    )
    wheels = sorted(arguments.dist.glob("openbnct-*.whl"))
    wheel = next((w for w in wheels if suffix in w.name), wheels[0] if wheels else None)
    if wheel is None:
        raise SystemExit(f"no openbnct wheel in {arguments.dist}")
    print(f"smoke-testing {wheel.name}")

    with tempfile.TemporaryDirectory() as tmp:
        env_dir = Path(tmp) / "venv"
        venv.create(env_dir, with_pip=True)
        python = env_dir / ("Scripts" if sys.platform == "win32" else "bin") / (
            "python.exe" if sys.platform == "win32" else "python"
        )
        command([python, "-m", "pip", "install", "--quiet", wheel])
        result = command(
            [
                python,
                "-c",
                "import openbnct; "
                "names=[n for n in dir(openbnct) if not n.startswith('_')]; "
                "assert 'Artifact' in names and 'DoseVolume' in names, names; "
                "print('openbnct import OK:', len(names), 'exports')",
            ]
        )
        print(result.stdout.strip())


if __name__ == "__main__":
    main()

# SPDX-License-Identifier: MIT

import io
from pathlib import Path
import runpy
import tarfile
import tempfile
import unittest
import zipfile


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
CHECK_SCRIPT = REPOSITORY_ROOT / "scripts" / "check_python_release_set.py"


def load_module() -> dict:
    return runpy.run_path(str(CHECK_SCRIPT))


def write_wheel(path: Path, members: dict[str, str]) -> None:
    with zipfile.ZipFile(path, "w") as archive:
        for name, text in members.items():
            archive.writestr(name, text)


def write_sdist(path: Path, members: dict[str, str]) -> None:
    with tarfile.open(path, "w:gz") as archive:
        for name, text in members.items():
            data = text.encode("utf-8")
            info = tarfile.TarInfo(name)
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))


class LicenseFilesTest(unittest.TestCase):
    def test_wheel_with_both_license_files_passes(self) -> None:
        module = load_module()
        with tempfile.TemporaryDirectory() as temporary:
            wheel = Path(temporary) / "openbnct-1.2.3-cp310-abi3-win_amd64.whl"
            write_wheel(
                wheel,
                {
                    "openbnct-1.2.3.dist-info/licenses/LICENSE": "MIT License",
                    "openbnct-1.2.3.dist-info/licenses/THIRD_PARTY_NOTICES.txt": "notices",
                },
            )
            self.assertEqual(module["wheel_license_errors"](wheel, "1.2.3"), [])

    def test_wheel_missing_or_empty_notices_fails(self) -> None:
        module = load_module()
        with tempfile.TemporaryDirectory() as temporary:
            missing = Path(temporary) / "missing.whl"
            write_wheel(missing, {"openbnct-1.2.3.dist-info/licenses/LICENSE": "MIT License"})
            empty = Path(temporary) / "empty.whl"
            write_wheel(
                empty,
                {
                    "openbnct-1.2.3.dist-info/licenses/LICENSE": "MIT License",
                    "openbnct-1.2.3.dist-info/licenses/THIRD_PARTY_NOTICES.txt": "",
                },
            )
            for wheel in (missing, empty):
                errors = module["wheel_license_errors"](wheel, "1.2.3")
                self.assertEqual(len(errors), 1, errors)
                self.assertIn("THIRD_PARTY_NOTICES.txt", errors[0])

    def test_sdist_license_files_are_checked_under_the_top_directory(self) -> None:
        module = load_module()
        with tempfile.TemporaryDirectory() as temporary:
            complete = Path(temporary) / "complete.tar.gz"
            write_sdist(
                complete,
                {"openbnct-1.2.3/LICENSE": "MIT License", "openbnct-1.2.3/THIRD_PARTY_NOTICES.txt": "notices"},
            )
            misplaced = Path(temporary) / "misplaced.tar.gz"
            write_sdist(misplaced, {"LICENSE": "MIT License", "openbnct-1.2.3/python/LICENSE": "MIT License"})

            self.assertEqual(module["sdist_license_errors"](complete, "1.2.3"), [])
            self.assertEqual(len(module["sdist_license_errors"](misplaced, "1.2.3")), 2)


if __name__ == "__main__":
    unittest.main()

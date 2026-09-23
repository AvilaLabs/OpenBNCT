# SPDX-License-Identifier: MIT

import argparse
from pathlib import Path
import runpy
import tempfile
import unittest


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
NOTICES_SCRIPT = REPOSITORY_ROOT / "scripts" / "third_party_notices.py"


def load_module() -> dict:
    """Execute the script and return its namespace, for direct unit testing.

    The script has no side effects at import time beyond defining functions
    (its work happens in `main()`, under the `__name__ == "__main__"`
    guard), so `runpy.run_path` is a cheap, dependency-free way to reach
    its pure functions without invoking cargo.
    """
    return runpy.run_path(str(NOTICES_SCRIPT))


def package(
    name: str,
    version: str,
    *,
    source: str | None = "registry+https://github.com/rust-lang/crates.io-index",
    license: str | None = "MIT",
    license_file: str | None = None,
    repository: str | None = "https://example.invalid/repo",
    manifest_dir: Path | None = None,
) -> dict:
    manifest_dir = manifest_dir or Path("/nonexistent")
    package_id = f"{name} {version} ({source or 'path+file:///workspace'})"
    return {
        "id": package_id,
        "name": name,
        "version": version,
        "source": source,
        "license": license,
        "license_file": license_file,
        "repository": repository,
        "manifest_path": str(manifest_dir / "Cargo.toml"),
    }


def node(pkg: dict, deps: list[tuple[dict, str | None]]) -> dict:
    """A resolve node for `pkg`, with one dep edge per (dep_pkg, kind) pair.

    `kind` is `None` for a normal edge, or `"dev"` / `"build"` — matching
    the real `dep_kinds[].kind` vocabulary cargo-metadata uses.
    """
    return {
        "id": pkg["id"],
        "deps": [{"pkg": dep_pkg["id"], "dep_kinds": [{"kind": kind, "target": None}]} for dep_pkg, kind in deps],
    }


class NormalDepIdsTest(unittest.TestCase):
    def test_keeps_only_normal_edges(self) -> None:
        normal_dep_ids = load_module()["normal_dep_ids"]
        alpha = package("alpha", "1.0.0")
        dev_only = package("dev-only", "9.9.9")
        build_only = package("buildtime-only", "3.3.3")
        resolve_node = node(package("root", "0.1.0"), [(alpha, None), (dev_only, "dev"), (build_only, "build")])

        self.assertEqual(normal_dep_ids(resolve_node), {alpha["id"]})

    def test_edge_present_as_both_normal_and_dev_counts_as_normal(self) -> None:
        normal_dep_ids = load_module()["normal_dep_ids"]
        alpha = package("alpha", "1.0.0")
        resolve_node = {
            "id": "root",
            "deps": [{"pkg": alpha["id"], "dep_kinds": [{"kind": "dev", "target": None}, {"kind": None, "target": None}]}],
        }

        self.assertEqual(normal_dep_ids(resolve_node), {alpha["id"]})


class WalkNormalGraphTest(unittest.TestCase):
    def test_traverses_through_path_dependencies_but_not_dev_edges(self) -> None:
        module = load_module()
        root = package("root", "0.1.0", source=None)
        local_helper = package("local-helper", "0.1.0", source=None)
        alpha = package("alpha", "1.0.0")
        beta = package("beta", "2.0.0")
        dev_only = package("dev-only", "9.9.9")
        build_only = package("buildtime-only", "3.3.3")

        metadata = {
            "resolve": {
                "nodes": [
                    node(root, [(alpha, None), (local_helper, None), (dev_only, "dev")]),
                    node(local_helper, [(beta, None), (build_only, "build")]),
                    node(alpha, []),
                    node(beta, []),
                ]
            }
        }

        reachable = module["walk_normal_graph"](metadata, [root["id"]])

        self.assertEqual(reachable, {root["id"], local_helper["id"], alpha["id"], beta["id"]})
        self.assertNotIn(dev_only["id"], reachable)
        self.assertNotIn(build_only["id"], reachable)


class ThirdPartyPackagesTest(unittest.TestCase):
    def test_excludes_path_sources_and_sorts_by_name_then_version(self) -> None:
        module = load_module()
        root = package("root", "0.1.0", source=None)
        local_helper = package("local-helper", "0.1.0", source=None)
        alpha_new = package("alpha", "2.0.0")
        alpha_old = package("alpha", "1.0.0")
        beta = package("beta", "1.0.0")

        metadata = {
            "packages": [root, local_helper, alpha_new, alpha_old, beta],
            "resolve": {
                "nodes": [
                    node(root, [(alpha_new, None), (local_helper, None)]),
                    node(local_helper, [(alpha_old, None), (beta, None)]),
                    node(alpha_new, []),
                    node(alpha_old, []),
                    node(beta, []),
                ]
            },
        }

        result = module["third_party_packages"](metadata, [root["id"]])

        self.assertEqual(
            [(p["name"], p["version"]) for p in result],
            [("alpha", "1.0.0"), ("alpha", "2.0.0"), ("beta", "1.0.0")],
        )


class DiscoverLicenseFilesTest(unittest.TestCase):
    def test_finds_root_license_files_and_every_font_text(self) -> None:
        module = load_module()
        with tempfile.TemporaryDirectory() as temporary:
            source_dir = Path(temporary)
            (source_dir / "LICENSE-MIT").write_text("mit text", encoding="utf-8")
            (source_dir / "LICENSE-APACHE").write_text("apache text", encoding="utf-8")
            (source_dir / "README.md").write_text("not a license", encoding="utf-8")
            (source_dir / "notes.txt").write_text("root .txt is not a license", encoding="utf-8")
            fonts_dir = source_dir / "fonts"
            fonts_dir.mkdir()
            (fonts_dir / "OFL.txt").write_text("ofl text", encoding="utf-8")
            (fonts_dir / "UFL.txt").write_text("ufl text", encoding="utf-8")
            # epaint_default_fonts ships the Hack license under this name.
            (fonts_dir / "Hack-Regular.txt").write_text("hack license text", encoding="utf-8")
            (fonts_dir / "Hack-Regular.ttf").write_bytes(b"\x00\x01font data")

            found = module["discover_license_files"](source_dir)

            self.assertEqual(
                [path.relative_to(source_dir).as_posix() for path in found],
                ["LICENSE-APACHE", "LICENSE-MIT", "fonts/Hack-Regular.txt", "fonts/OFL.txt", "fonts/UFL.txt"],
            )

    def test_missing_source_directory_returns_no_files(self) -> None:
        module = load_module()
        self.assertEqual(module["discover_license_files"](Path("/nonexistent/path")), [])


class LicenseTextPoolTest(unittest.TestCase):
    def test_deduplicates_identical_text_by_hash(self) -> None:
        pool = load_module()["LicenseTextPool"]()

        first_id = pool.add("same license text\n")
        second_id = pool.add("same license text\n")
        third_id = pool.add("a different license text\n")

        self.assertEqual(first_id, second_id)
        self.assertNotEqual(first_id, third_id)
        self.assertEqual(len(pool.entries), 2)
        self.assertEqual(pool.entries[0][0], first_id)
        self.assertEqual(pool.entries[1][0], third_id)


class DescribeLicenseTest(unittest.TestCase):
    def test_prefers_spdx_expression(self) -> None:
        describe_license = load_module()["describe_license"]
        self.assertEqual(describe_license({"license": "MIT OR Apache-2.0", "license_file": None}), "MIT OR Apache-2.0")

    def test_falls_back_to_license_file(self) -> None:
        describe_license = load_module()["describe_license"]
        self.assertEqual(describe_license({"license": None, "license_file": "LICENSE-CUSTOM"}), "see LICENSE-CUSTOM")

    def test_unspecified_when_neither_present(self) -> None:
        describe_license = load_module()["describe_license"]
        self.assertEqual(describe_license({"license": None, "license_file": None}), "UNSPECIFIED")


class ParseExtraTest(unittest.TestCase):
    def test_splits_name_and_file(self) -> None:
        parse_extra = load_module()["parse_extra"]
        self.assertEqual(parse_extra("Noto Sans CJK JP (subset)=assets/font.LICENSE.txt"), ("Noto Sans CJK JP (subset)", Path("assets/font.LICENSE.txt")))

    def test_rejects_value_without_equals(self) -> None:
        parse_extra = load_module()["parse_extra"]
        with self.assertRaises(argparse.ArgumentTypeError):
            parse_extra("no-equals-sign")


class RenderReportTest(unittest.TestCase):
    def _build_metadata(self, alpha_dir: Path, beta_dir: Path, gamma_dir: Path) -> tuple[dict, dict]:
        root = package("root", "0.1.0", source=None, manifest_dir=Path("/workspace/root"))
        local_helper = package("local-helper", "0.1.0", source=None, manifest_dir=Path("/workspace/local-helper"))
        alpha = package("alpha", "1.0.0", license="MIT", manifest_dir=alpha_dir)
        beta = package("beta", "2.0.0", license="MIT OR Apache-2.0", manifest_dir=beta_dir)
        gamma = package("gamma", "0.5.0", license="Apache-2.0", manifest_dir=gamma_dir)
        dev_only = package("dev-only", "9.9.9")
        build_only = package("buildtime-only", "3.3.3")

        metadata = {
            "packages": [root, local_helper, alpha, beta, gamma, dev_only, build_only],
            "workspace_members": [root["id"]],
            "resolve": {
                "root": None,
                "nodes": [
                    node(root, [(alpha, None), (local_helper, None), (gamma, None), (dev_only, "dev")]),
                    node(local_helper, [(beta, None), (build_only, "build")]),
                    node(alpha, []),
                    node(beta, []),
                    node(gamma, []),
                ],
            },
        }
        return metadata, {"root": root}

    def test_deterministic_and_deduplicates_shared_license_text(self) -> None:
        module = load_module()
        shared_mit_text = "MIT License text shared verbatim by two crates\n"

        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            alpha_dir, beta_dir, gamma_dir = base / "alpha", base / "beta", base / "gamma"
            alpha_dir.mkdir()
            beta_dir.mkdir()
            gamma_dir.mkdir()  # gamma ships no license file at all

            (alpha_dir / "LICENSE-MIT").write_text(shared_mit_text, encoding="utf-8")
            (beta_dir / "LICENSE-MIT").write_text(shared_mit_text, encoding="utf-8")
            (beta_dir / "LICENSE-APACHE").write_text("Apache license text, distinct\n", encoding="utf-8")

            metadata, _ = self._build_metadata(alpha_dir, beta_dir, gamma_dir)

            first = module["render_report"](metadata, ["root"], [])
            second = module["render_report"](metadata, ["root"], [])

        self.assertEqual(first, second, "rendering the same metadata twice must be byte-identical")

        # Dev- and build-only dependencies never appear.
        self.assertNotIn("dev-only", first)
        self.assertNotIn("buildtime-only", first)
        # Local path packages are traversed through, never listed themselves.
        self.assertNotIn("local-helper", first)

        # alpha and beta both ship the identical MIT text — it is stored once.
        self.assertEqual(first.count(shared_mit_text), 1)
        self.assertEqual(first.count("Apache license text, distinct"), 1)

        # gamma has no bundled license file; the fallback note names its SPDX expression.
        self.assertIn("not bundled with this package", first)
        self.assertIn("the standard text of Apache-2.0 applies.", first)

        # Sorted by name: alpha before beta before gamma.
        self.assertLess(first.index("alpha 1.0.0"), first.index("beta 2.0.0"))
        self.assertLess(first.index("beta 2.0.0"), first.index("gamma 0.5.0"))

    def test_extra_component_is_appended_with_its_own_license_text(self) -> None:
        module = load_module()

        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            alpha_dir, beta_dir, gamma_dir = base / "alpha", base / "beta", base / "gamma"
            for path in (alpha_dir, beta_dir, gamma_dir):
                path.mkdir()
            extra_license = base / "NotoSansCJKjp-UI.LICENSE.txt"
            extra_license.write_text("SIL Open Font License text\n", encoding="utf-8")

            metadata, _ = self._build_metadata(alpha_dir, beta_dir, gamma_dir)

            report = module["render_report"](metadata, ["root"], [("Noto Sans CJK JP (subset)", extra_license)])

        self.assertIn("ADDITIONAL COMPONENTS", report)
        self.assertIn("Noto Sans CJK JP (subset)", report)
        self.assertIn("SIL Open Font License text", report)


if __name__ == "__main__":
    unittest.main()

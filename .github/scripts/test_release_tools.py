# Project:  Privatium™  |  File: .github/scripts/test_release_tools.py
# Authors:  Gabriel Mongefranco (@gabrielmongefranco)
# Created:  2026-09-05  |  Modified: 2026-09-06
# Summary:  Verify release archive layout and the exact-commit CI prerequisite.

import tempfile
import unittest
import zipfile
import tarfile
from pathlib import Path

from release_tools import EXAMPLE_APPS, package, package_portable, require_ci


class ReleaseTests(unittest.TestCase):
    def test_portable_zip_holds_the_binary_the_readme_and_the_example_apps(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "release"
            source.mkdir()
            (source / "privatium.exe").write_bytes(b"synthetic binary\x00\xff")
            apps = root / "apps"
            for slug in EXAMPLE_APPS:
                (apps / slug / "views").mkdir(parents=True)
                (apps / slug / "app.toml").write_text(f'[app]\nslug = "{slug}"\n')
                (apps / slug / "views" / "index.lsp").write_text("<h1>x</h1>")
            (apps / "_lint" / "fail").mkdir(parents=True)
            (apps / "_lint" / "fail" / "app.toml").write_text("excluded")
            (apps / "README.md").write_text("excluded")
            result = package_portable(source, root / "dist", apps)
            self.assertEqual(result.name, "privatium-windows-portable.zip")
            with zipfile.ZipFile(result) as bundle:
                names = bundle.namelist()
                self.assertEqual(names[0], "privatium.exe")
                self.assertEqual(bundle.getinfo("privatium.exe").external_attr >> 16 & 0o777, 0o755)
                self.assertIn("README.txt", names)
                self.assertIn("privatium-data", bundle.read("README.txt").decode())
                for slug in EXAMPLE_APPS:
                    self.assertIn(f"privatium-data/apps/{slug}/app.toml", names)
                    self.assertIn(f"privatium-data/apps/{slug}/views/index.lsp", names)
                self.assertFalse(any("_lint" in n or n.endswith("apps/README.md") for n in names), names)
                self.assertEqual(len(names), 2 + 2 * len(EXAMPLE_APPS))
            with self.assertRaises(FileNotFoundError):
                package_portable(source, root / "dist", root / "no-apps")
            with self.assertRaises(FileNotFoundError):
                package_portable(root / "no-binary", root / "dist", apps)

    def test_archives_contain_only_the_original_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for os_name, archive, binary in [
                ("Linux", "privatium-linux.tar.gz", "privatium"),
                ("macOS", "privatium-mac.zip", "privatium"),
                ("Windows", "privatium-windows.zip", "privatium.exe"),
            ]:
                source = root / os_name
                source.mkdir()
                (source / binary).write_bytes(b"synthetic binary\x00\xff")
                (source / "unrelated.pdb").write_text("excluded")
                result = package(os_name, source, root / "dist")
                self.assertEqual(result.name, archive)
                if os_name == "Linux":
                    with tarfile.open(result) as bundle:
                        self.assertEqual(bundle.getnames(), [binary])
                        self.assertEqual(bundle.extractfile(binary).read(), (source / binary).read_bytes())
                        self.assertEqual(bundle.getmember(binary).mode, 0o755)
                else:
                    with zipfile.ZipFile(result) as bundle:
                        self.assertEqual(bundle.namelist(), [binary])
                        self.assertEqual(bundle.read(binary), (source / binary).read_bytes())
                        self.assertEqual(bundle.getinfo(binary).external_attr >> 16 & 0o777, 0o755)

    def test_package_refuses_unknown_platform_and_missing_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaises(ValueError):
                package("../other", root, root / "dist")
            with self.assertRaises(FileNotFoundError):
                package("Linux", root, root / "dist")

    def test_ci_requires_latest_matching_push_to_succeed(self):
        good = dict(id=1, head_sha="a" * 40, event="push", status="completed", conclusion="success")
        require_ci([good], "a" * 40)
        for runs in [[], [dict(good, head_sha="b" * 40)], [dict(good, event="pull_request")],
                     [dict(good, conclusion="failure")], [dict(good, status="in_progress")],
                     [good, dict(good, id=2, conclusion="cancelled")]]:
            with self.subTest(runs=runs), self.assertRaises(ValueError):
                require_ci(runs, "a" * 40)


if __name__ == "__main__":
    unittest.main()

# Project:  Privatium™  |  File: .github/scripts/test_release_tools.py
# Authors:  Gabriel Mongefranco (@gabrielmongefranco)
# Created:  2026-09-05  |  Modified: 2026-09-05
# Summary:  Verify release archive layout and the exact-commit CI prerequisite.

import tempfile
import unittest
import zipfile
import tarfile
from pathlib import Path

from release_tools import package, require_ci


class ReleaseTests(unittest.TestCase):
    def test_archives_contain_only_the_original_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for os_name, archive, binary in [
                ("Linux", "privatium.linux.tar.gz", "privatium"),
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

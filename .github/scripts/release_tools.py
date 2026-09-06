# Project:  Privatium™  |  File: .github/scripts/release_tools.py
# Authors:  Gabriel Mongefranco (@gabrielmongefranco)
# Created:  2026-09-05  |  Modified: 2026-09-06
# Summary:  Package a single executable — and for Windows a portable zip holding the
#           privatium-data folder with the example apps — and require successful CI
#           for a release commit.

import argparse
import json
import os
import re
import subprocess
import tarfile
import zipfile
from pathlib import Path

# The example apps of apps/README.md, the folders a portable zip carries under
# privatium-data/apps/ so its launcher is never empty (spec/cli.md §1, §2).
EXAMPLE_APPS = ("hello", "animals", "sketch")

PORTABLE_README = """Privatium, portable.

Keep privatium.exe and the privatium-data folder together: while that folder sits
beside the program, everything Privatium knows lives in it and nothing is written
anywhere else on this computer (spec/cli.md section 1).

  privatium-data\\apps      the apps; three examples to start with
  privatium-data\\data      your information, as plain text files
  privatium-data\\identity  this node's keys; keep private

Back up by copying this whole folder. Move it, and the program follows.
Delete privatium-data (after copying it) to use the platform data directory instead.
"""


def package(platform, source, destination):
    """Archive only the platform binary, preserving an executable Unix mode."""
    names = {"Linux": ("privatium-linux.tar.gz", "privatium"),
             "macOS": ("privatium-mac.zip", "privatium"),
             "Windows": ("privatium-windows.zip", "privatium.exe")}
    if platform not in names:
        raise ValueError("Cannot package this platform; use Linux, macOS or Windows.")
    archive, binary = names[platform]
    path = source / binary
    if not path.is_file() or path.is_symlink():
        raise FileNotFoundError("Cannot package the binary; build the release executable first.")
    destination.mkdir(parents=True, exist_ok=True)
    result = destination / archive
    if platform == "Linux":
        with tarfile.open(result, "w:gz") as bundle, path.open("rb") as executable:
            entry = bundle.gettarinfo(str(path), arcname=binary)
            entry.mode = 0o755
            entry.uid = entry.gid = 0
            entry.uname = entry.gname = ""
            bundle.addfile(entry, executable)
    else:
        with zipfile.ZipFile(result, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
            entry = zipfile.ZipInfo(binary)
            entry.create_system = 3
            entry.external_attr = 0o100755 << 16
            entry.compress_type = zipfile.ZIP_DEFLATED
            bundle.writestr(entry, path.read_bytes())
    return result


def package_portable(source, destination, apps):
    """Zip the Windows binary with a privatium-data folder holding the example apps."""
    binary = "privatium.exe"
    path = source / binary
    if not path.is_file() or path.is_symlink():
        raise FileNotFoundError("Cannot package the binary; build the release executable first.")
    for slug in EXAMPLE_APPS:
        if not (apps / slug / "app.toml").is_file():
            raise FileNotFoundError(f"Cannot package the portable zip; apps/{slug} is not an app folder.")
    destination.mkdir(parents=True, exist_ok=True)
    result = destination / "privatium-windows-portable.zip"
    with zipfile.ZipFile(result, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
        entry = zipfile.ZipInfo(binary)
        entry.create_system = 3
        entry.external_attr = 0o100755 << 16
        entry.compress_type = zipfile.ZIP_DEFLATED
        bundle.writestr(entry, path.read_bytes())
        bundle.writestr("README.txt", PORTABLE_README)
        for slug in EXAMPLE_APPS:
            folder = apps / slug
            for file in sorted(p for p in folder.rglob("*") if p.is_file()):
                arcname = "privatium-data/apps/" + file.relative_to(apps).as_posix()
                bundle.write(file, arcname)
    return result


def require_ci(runs, sha):
    """Refuse unless the latest push CI run for this exact commit completed successfully."""
    matching = [run for run in runs if run.get("head_sha") == sha and run.get("event") == "push"]
    latest = max(matching, key=lambda run: run["id"], default={})
    if latest.get("status") != "completed" or latest.get("conclusion") != "success":
        raise ValueError("Release build refused: let push CI pass for the release commit, then rerun this workflow.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["package", "check-ci"])
    parser.add_argument("value")
    args = parser.parse_args()
    if args.command == "package":
        results = [package(args.value, Path("target/release"), Path("dist"))]
        if args.value == "Windows":
            results.append(package_portable(Path("target/release"), Path("dist"), Path("apps")))
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
            # One path per line: actions/upload-artifact reads a multi-line `path`.
            output.write("path<<PATHS\n" + "".join(f"{r.as_posix()}\n" for r in results) + "PATHS\n")
    else:
        if not re.fullmatch(r"[0-9a-f]{40}", args.value):
            parser.error("Expected the release commit SHA.")
        repo = os.environ["GITHUB_REPOSITORY"]
        response = subprocess.check_output([
            "gh", "api", "--method", "GET", "--paginate", "--slurp",
            f"repos/{repo}/actions/workflows/ci.yml/runs",
            "-f", f"head_sha={args.value}", "-f", "event=push", "-f", "per_page=100",
        ], text=True)
        require_ci([run for page in json.loads(response) for run in page["workflow_runs"]], args.value)
        print("Latest push CI passed for the release commit.")


if __name__ == "__main__":
    main()

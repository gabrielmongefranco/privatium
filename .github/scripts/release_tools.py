# Project:  Privatium™  |  File: .github/scripts/release_tools.py
# Authors:  Gabriel Mongefranco (@gabrielmongefranco)
# Created:  2026-09-05  |  Modified: 2026-09-05
# Summary:  Package a single executable and require successful CI for a release commit.

import argparse
import json
import os
import re
import subprocess
import tarfile
import zipfile
from pathlib import Path


def package(platform, source, destination):
    """Archive only the platform binary, preserving an executable Unix mode."""
    names = {"Linux": ("privatium.linux.tar.gz", "privatium"),
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
        result = package(args.value, Path("target/release"), Path("dist"))
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
            output.write(f"path={result.as_posix()}\n")
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

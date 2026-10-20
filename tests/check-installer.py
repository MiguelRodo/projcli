#!/usr/bin/env python3
"""Exercise actual CI artifact installation and fail-closed replacement paths."""
import argparse
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--shell", choices=["bash", "pwsh"], required=True)
parser.add_argument("--fixture", type=Path, required=True)
args = parser.parse_args()
installer = Path("install.ps1" if args.shell == "pwsh" else "install-local.sh").resolve()
suffix = ".exe" if args.shell == "pwsh" else ""
platform = "windows_amd64.exe" if suffix else "linux_amd64"
original_fixture = args.fixture.resolve()
for case in ["missing-checksum", "wrong-checksum", "corrupt-binary", "duplicate-checksum", "wrong-version", "valid"]:
    with tempfile.TemporaryDirectory(prefix="proj installer ") as temporary:
        root = Path(temporary)
        fixture = root / "release assets"
        shutil.copytree(original_fixture, fixture)
        target_directory = root / "user bin"
        target_directory.mkdir()
        target = target_directory / f"proj{suffix}"
        target.write_bytes(b"previous installed bytes")
        before = target.read_bytes()
        checksum = fixture / "checksums.txt"
        asset, = fixture.glob(f"proj_*_{platform}")
        if case == "missing-checksum": checksum.unlink()
        elif case == "wrong-checksum": checksum.write_text(f"{'0' * 64}  {asset.name}\n")
        elif case == "corrupt-binary": asset.write_bytes(b"corrupt download")
        elif case == "duplicate-checksum": checksum.write_text(checksum.read_text() * 2)
        environment = os.environ.copy()
        environment.update(PROJ_DOWNLOAD_BASE_URL=fixture.as_uri(), PROJ_INSTALL_DIR=str(target_directory),
                           PROJ_RELEASE_VERSION="v0.0.0-wrong" if case == "wrong-version" else (fixture / "version.txt").read_text().strip(),
                           XDG_STATE_HOME=str(root / "state"), LOCALAPPDATA=str(root / "state"))
        command = [args.shell] + (["-NoProfile", "-File"] if args.shell == "pwsh" else []) + [str(installer)]
        output = subprocess.run(command, env=environment, capture_output=True, text=True)
        if case == "valid":
            assert output.returncode == 0, (output.stdout, output.stderr)
            assert target.read_bytes() == asset.read_bytes()
            subprocess.run([str(target), "--help"], check=True, capture_output=True)
            subprocess.run([str(target), "--version"], check=True, capture_output=True)
        else:
            assert output.returncode != 0, case
            assert target.read_bytes() == before, case
        print(f"{args.shell}: {case} passed")

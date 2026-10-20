#!/usr/bin/env python3
"""Stage/collect preview binaries with version-bound SHA256 manifests (stdlib only)."""
import argparse
import hashlib
import re
import shutil
import subprocess
import tomllib
from pathlib import Path

PLATFORMS = {"linux_amd64": "", "windows_amd64": ".exe"}

def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()

def version():
    value = tomllib.loads(Path("Cargo.toml").read_text())["package"]["version"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?(?:\+[A-Za-z0-9.-]+)?", value):
        raise ValueError("Unsupported release version")
    return value

def asset_name(value, platform):
    return f"proj_{value}_{platform}{PLATFORMS[platform]}"

def verify_tag_checkout(tag, repository=None):
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+-preview\.[0-9]+", tag):
        raise ValueError("Expected a preview tag")
    def rev(reference):
        return subprocess.check_output(["git", "rev-parse", "--verify", reference], cwd=repository, text=True).strip()
    commit = rev("HEAD")
    if rev(f"refs/tags/{tag}^{{commit}}") != commit:
        raise ValueError("Checkout commit differs from qualified tag")
    return commit

def stage(binary, platform, output, tag=None, commit=None):
    value = version()
    if tag is not None and tag != f"v{value}":
        raise ValueError("Tag and Cargo version differ")
    if not binary.is_file():
        raise ValueError("Built binary missing")
    output.mkdir(parents=True, exist_ok=True)
    target = output / asset_name(value, platform)
    shutil.copy2(binary, target)
    (output / "version.txt").write_text(value + "\n")
    if commit is not None:
        if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", commit): raise ValueError("Invalid source commit")
        (output / "source-commit.txt").write_text(commit + "\n")
    if tag is not None and commit is None: raise ValueError("Tagged artifacts require source commit identity")
    (output / "checksums.txt").write_text(f"{digest(target)}  {target.name}\n")

def collect(source, output, tag, commit):
    value = version()
    if tag != f"v{value}":
        raise ValueError("Tag and Cargo version differ")
    output.mkdir(parents=True, exist_ok=False)
    checksums = []
    for platform in PLATFORMS:
        candidates = list(source.rglob(asset_name(value, platform)))
        if len(candidates) != 1:
            raise ValueError(f"Expected exactly one {platform} binary")
        binary = candidates[0]
        if (binary.parent / "version.txt").read_text().strip() != value:
            raise ValueError("Artifact version metadata differs")
        if (binary.parent / "source-commit.txt").read_text().strip() != commit:
            raise ValueError("Platform artifact source commit differs from checked-out tag")
        expected = f"{digest(binary)}  {binary.name}\n"
        if (binary.parent / "checksums.txt").read_text() != expected:
            raise ValueError("Artifact checksum mismatch")
        shutil.copy2(binary, output / binary.name)
        checksums.append(expected)
    (output / "version.txt").write_text(value + "\n")
    (output / "source-commit.txt").write_text(commit + "\n")
    for name in ["version.txt", "source-commit.txt"]:
        checksums.append(f"{digest(output / name)}  {name}\n")
    (output / "checksums.txt").write_text("".join(checksums))

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    command = commands.add_parser("stage")
    command.add_argument("--binary", type=Path, required=True)
    command.add_argument("--platform", choices=PLATFORMS, required=True)
    command.add_argument("--output", type=Path, required=True)
    command.add_argument("--tag")
    command.add_argument("--commit")
    command = commands.add_parser("collect")
    command.add_argument("--input", type=Path, required=True)
    command.add_argument("--output", type=Path, required=True)
    command.add_argument("--tag", required=True)
    command.add_argument("--commit", required=True)
    command = commands.add_parser("verify-tag")
    command.add_argument("--tag", required=True)
    args = parser.parse_args()
    if args.command == "stage": stage(args.binary, args.platform, args.output, args.tag, args.commit)
    elif args.command == "collect": collect(args.input, args.output, args.tag, args.commit)
    else: print(verify_tag_checkout(args.tag))

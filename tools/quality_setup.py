"""Install checksum-pinned development tools only in the supplied CI temp directory."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import urllib.error
import urllib.request

AIR_URL = "https://github.com/posit-dev/air/releases/download/0.8.2/air-x86_64-unknown-linux-gnu.tar.gz"
AIR_SHA = "94d03a402d0a86947f9093f0db015a627d4f2035c23f4ab110d01bb363855704"
PACKAGES = [
    ("lazyeval", "0.2.2", "d6904112a21056222cfcd5eb8175a78aa063afe648a562d9c42c6b960a8820d4"),
    ("rex", "1.2.1", "af42e649c06e4bbdba94d5a1870a7e8347903571c90cd5e5ca40f52307a3bfd6"),
    ("xmlparsedata", "1.0.5", "766034ab5e9728609bd240c9954d23ca0cdb881a98a31b9d3e1c8767c7b7cbb0"),
    ("lintr", "3.4.0", "54144e68b70289edff7eed4fc8d00efacd1ddd51c032dc1e88bfb657c10a89dc"),
]


def fetch(url, expected):
    data = urllib.request.urlopen(url, timeout=90).read()
    if hashlib.sha256(data).hexdigest() != expected:
        raise RuntimeError("Pinned development-tool checksum mismatch")
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tools", required=True, type=Path)
    parser.add_argument("--selection", required=True, type=Path)
    args = parser.parse_args()
    selected = json.loads(args.selection.read_text())
    if not selected["r"]:
        print("No changed R source: Air/lintr setup skipped.")
        return
    args.tools.mkdir(parents=True, exist_ok=True)
    archive = tarfile.open(fileobj=io.BytesIO(fetch(AIR_URL, AIR_SHA)))
    members = [member for member in archive.getmembers()
               if Path(member.name).name == "air" and member.isfile()]
    if len(members) != 1 or members[0].size > 100 * 1024 * 1024:
        raise RuntimeError("Unexpected official Air archive")
    binary = args.tools / "air"
    binary.write_bytes(archive.extractfile(members[0]).read())
    binary.chmod(0o755)
    subprocess.run([str(binary), "--version"], check=True)
    library = args.tools / "R-library"
    library.mkdir(exist_ok=True)
    environment = dict(os.environ, R_LIBS_USER=str(library),
                       R_PROFILE_USER="/dev/null", R_ENVIRON_USER="/dev/null")
    for name, version, sha in PACKAGES:
        prefix = "https://cran.r-project.org/src/contrib"
        try:
            data = fetch(f"{prefix}/{name}_{version}.tar.gz", sha)
        except urllib.error.HTTPError as error:
            if error.code != 404:
                raise
            data = fetch(f"{prefix}/Archive/{name}/{name}_{version}.tar.gz", sha)
        source = args.tools / f"{name}_{version}.tar.gz"
        source.write_bytes(data)
        subprocess.run(["R", "CMD", "INSTALL", "--no-multiarch",
                        f"--library={library}", str(source)], env=environment, check=True)
    subprocess.run(["Rscript", "--vanilla", "-e",
                    'stopifnot(as.character(packageVersion("lintr")) == "3.4.0")'],
                   env=environment, check=True)


if __name__ == "__main__":
    main()

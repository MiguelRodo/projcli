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

AIR_URL = "https://github.com/posit-dev/air/releases/download/0.12.0/air-x86_64-unknown-linux-gnu.tar.gz"
AIR_SHA = "d2b9452bfc44d55f0e61c2d8906ca3fbaac5e63f40726017acf8bad6bfd3ba37"
PACKAGES = [
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
    # CRAN rex 1.2.1 pulls lazyeval, whose C API cannot load on the pinned R 4.6.
    # Official upstream rex removes that dependency; use an immutable reviewed pin.
    rex_url = "https://codeload.github.com/r-lib/rex/tar.gz/7a8fb36a29919c3d447cc823c672af9b5cf1fd68"
    rex_sha = "ecc117587a7d1cd970d2cfac9d038eeafcfa9036268ac31354eda987b9af812e"
    rex_source = args.tools / "rex-pinned.tar.gz"
    rex_source.write_bytes(fetch(rex_url, rex_sha))
    subprocess.run(["R", "CMD", "INSTALL", "--no-multiarch",
                    f"--library={library}", str(rex_source)], env=environment, check=True)
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

    # Object-usage lint needs the candidate namespace, not an installed old release.
    # Existing Imports must already exist in the pinned image; no package updates.
    if any(Path(name).parts[:3] == ("frontends", "r", "projcli") for name in selected["r"]):
        package = Path(__file__).resolve().parents[1] / "frontends/r/projcli"
        subprocess.run(["R", "CMD", "INSTALL", "--no-multiarch",
                        f"--library={library}", str(package)], env=environment, check=True)
        probe = r"""
        probe <- tempfile("lint-context-", tmpdir="frontends/r/projcli/R", fileext=".R")
        tryCatch({
          writeLines('probe <- function() .proj_error("fixture", "fixture")', probe)
          stopifnot(length(lintr::lint(probe, linters=lintr::object_usage_linter(), cache=FALSE)) == 0L)
          writeLines('probe <- function() .proj_lint_unknown()', probe)
          findings <- lintr::lint(probe, linters=lintr::object_usage_linter(), cache=FALSE)
          stopifnot(any(vapply(findings, function(x) grepl(".proj_lint_unknown", x$message, fixed=TRUE), logical(1))))
          cat("PASS candidate namespace context and undefined-helper detection\n")
        }, finally=unlink(probe))
        """
        subprocess.run(["Rscript", "--vanilla", "-e", probe], env=environment, check=True)


if __name__ == "__main__":
    main()

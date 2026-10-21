# Development and renderer diagnostics

`.devcontainer/devcontainer.json` pins the published `v1.0.0` image and its
immutable registry digest:

```text
ghcr.io/miguelrodo/devcontainers-projcli:v1.0.0@sha256:a58a8a4dd6f0be83879d08b3a43ab92e256425bbfb81372b257d77a7ff9077f9
```

The [image definition](https://github.com/MiguelRodo/devcontainers/tree/78bb081f077a974f3179b0a8a60c7efbc0f1e79a/.devcontainer)
uses the repository's established Bioconductor 3.23 base and non-root `rstudio`
user. It supplies stable Rust, C/C++ build tools, Python 3/venv, knitr/rmarkdown,
Quarto with bundled Pandoc/TinyTeX, and qpdf/Poppler for actual image/PDF inspection.
The standard Rust feature's extra container permissions are avoided.

## Verified remote environment

The [normal prebuild publication](https://github.com/MiguelRodo/devcontainers/actions/runs/39564970855)
built source `78bb081f077a974f3179b0a8a60c7efbc0f1e79a`, injected version `v1.0.0`
and pushed the digest above. The [registry-image verification](https://github.com/MiguelRodo/devcontainers/actions/runs/39565799754)
selected that immutable image without adding features, then passed the complete
smoke and fresh nonempty `PASS.txt` witness. Artifacts retain commands, named
versions, relocated HTML/PDF and extracted PDF image pixels.

The smoke reads sibling CSV data with Rust/Python/R and renders R Markdown plus
Quarto to embedded HTML/PDF. It checks exact embedded PNG bytes, PDF structure,
CSV-derived text and decoded PDF image pixels after relocating reports and
hiding the entire source tree. TeX package auto-install is disabled during renders.
This validates installed tools on Linux x86-64; it does not establish projcli path
behavior, R/IDE/projr parity or production readiness.

Observed versions: Rust/Cargo 1.99.1 stable, Python 3.12.3, R 4.6.1, knitr 1.52,
rmarkdown 2.32, Quarto 1.10.19, Pandoc 3.10 and TinyTeX v2026.10. `container-info`
reported `v1.0.0`. A later rebuild can resolve newer stable Rust/TeX/features;
use a new SemVer image and verified digest rather than reusing a published version.

## Usage and maintenance boundaries

No project build or dependency installation runs on container creation. No
Docker socket, host home, extra capabilities, privileged mode or authentication
state is configured. Codex installation and login are the user's later step.
Local VS Code/Positron devcontainer usability has not been verified. While local
memory is constrained, keep Docker builds, Rust compilation, dependency-heavy
checks and R/Quarto/PDF renders in remote Actions.

Image updates use the existing prebuild workflow on the long-lived `projcli`
branch, with monthly Dependabot maintenance configured in devcontainers `main`.
The pinned publisher's JSONC comment stripper requires JSON-escaped slashes in
the CRAN URL; they decode to the identical mirror. This limitation and the
serialization-only fix are documented in [devcontainers PR15](https://github.com/MiguelRodo/devcontainers/pull/15).
No alternative publisher, credentials or visibility changes were introduced.

SATVILab/projr remains read-only. No copy, patch or installation of it is part of
this image. Unmerged diagnostic source
`488adaf727668bdd6c3f46c544e3801ce92de24b` informed the narrow image checks without
being merged wholesale. Later comparisons must use unchanged pinned remote source
and record differences in `docs/compatibility.qmd`.

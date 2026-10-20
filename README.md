# projcli

Run a research analysis from a small project configuration. Keep the steps and
paths together, preview changes while you work, and prepare versioned outputs
when you are ready.

**Status:** the `main` branch is a development preview. The published v0.3.0
release predates recent preview-safety and project-root changes. Its version
number alone does not identify those features.

[Website](https://miguelrodo.github.io/projcli/) ·
[What works](https://miguelrodo.github.io/projcli/features.html) ·
[Build reference](docs/cli/build.qmd)

## Installation

The current preview's release and installer routes are being repaired and
verified. The [published v0.3.0 release](https://github.com/MiguelRodo/projcli/releases/tag/v0.3.0)
currently has one amd64 Debian package; it is not the preview described below.

For a source build you need Git, a current stable Rust toolchain with Cargo,
and platform linker tools (for example Ubuntu's `build-essential`).
This source-install command is a candidate awaiting installation verification:

```bash
cargo install --git https://github.com/MiguelRodo/projcli --locked --bin proj
```

Cargo normally installs to `~/.cargo/bin`, which must be on your `PATH`.
Record the source commit from the install output. Check `command -v proj`
and `proj --help`: another program may use the same command name.
Current-preview help includes `build-dev`, `--project` and `--non-interactive`.

Use Linux commands inside Ubuntu when working in WSL. Native Windows,
macOS and ARM release installers are not yet verified for this preview.
See [installation details](docs/install.qmd) for prerequisites and limits.

## Quickstart: a small Python analysis

This example targets the current development preview and still needs checking
against the next release. You need Python 3 available as `python3`.

In a new folder, create `_proj.yml`:

```yaml
directories:
  cache:
    path: _tmp
build:
  scripts: [analysis.py]
```

Create `analysis.py` beside it:

```python
import os
from pathlib import Path
from statistics import mean

root = Path(os.environ["PROJ_PROJECT_ROOT"])
preview = root / "_tmp" / "preview"
preview.mkdir(parents=True, exist_ok=True)
result = preview / "mean.txt"
result.write_text(f"{mean([10, 12, 14])}\n")
print(f"Saved {result}")
```

From that folder, run:

```bash
proj --project . yml read
proj --project . --non-interactive build-dev
cat _tmp/preview/mean.txt
```

The result should be `12`. The CLI does not create a project version or publish
outputs during this preview. Scripts still control their writes: a preview is
not a filesystem sandbox.

The [full tutorial](docs/tutorial.qmd) saves a CSV and explains each step.
Read the [build reference](docs/cli/build.qmd) before using production
`proj build`, which has different version, cleanup and export behavior.

## More detail

- [Configuration](docs/configuration.qmd) and [command reference](docs/cli/build.qmd).
- [Behavior contract](docs/behavior.qmd): intended behavior and known gaps.
- [Compatibility log](docs/compatibility.qmd): comparison with `projr`.

The existing R package is unchanged. Matching CLI, RStudio and GUI renderer
behavior has not yet been proved; new frontend integrations are separate work.

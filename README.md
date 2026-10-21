# projcli

Run a research analysis from a small project configuration. Keep the steps and
paths together and explore the documented preview workflow.

> [!WARNING]
> Use sample data or a disposable copy of your project. Production builds
> (`proj build`) can modify or remove files, and safety fixes are still pending.
> **Never run production builds on the only copy of important work.**
> Previews are not a security sandbox; code you run can still change files.

**Status:** [v0.4.0-preview.1](https://github.com/MiguelRodo/projcli/releases/tag/v0.4.0-preview.1)
is a development prerelease, not completion of M1/M2. Stable Latest remains
v0.3.0 and predates the current preview features.

The next preview is still being checked. Its [release and installation checklist](docs/release-candidate.qmd)
separates newer source changes from the binary available below. Installing this
version does not install the pending R-data or HTML-default changes.

[Website](https://miguelrodo.github.io/projcli/) ·
[What works](https://miguelrodo.github.io/projcli/features.html) ·
[Build reference](docs/cli/build.qmd)

## Installation

Use the pinned [v0.4.0-preview.1 development release](https://github.com/MiguelRodo/projcli/releases/tag/v0.4.0-preview.1).
It supplies Linux x86-64 (glibc 2.35 or newer) and Windows x86-64 binaries.
**No Rust compiler or administrator access is needed.** macOS and ARM binaries
are not supplied.

For Ubuntu / WSL Ubuntu, you need Bash, `curl` and `sha256sum` or `shasum`.
Run this inside Ubuntu:

```bash
mkdir -p "$HOME/.local/bin"
installer="$(mktemp)"
curl -fsSL "https://raw.githubusercontent.com/MiguelRodo/projcli/bbd12edc8de9865a96b6145d0c7702f1300cb1a8/install-local.sh" -o "$installer" &&
  PROJ_RELEASE_VERSION=0.4.0-preview.1 \
  PROJ_INSTALL_DIR="$HOME/.local/bin" bash "$installer"
rm -f "$installer"
export PATH="$HOME/.local/bin:$PATH"
hash -r
proj --version
```

Expected version: `proj 0.4.0-preview.1`. The installer verifies SHA256 before
replacing `~/.local/bin/proj`. The PATH change applies to the current terminal.
The explicit version pin matters: GitHub's stable **Latest** is still v0.3.0.

See [the install guide](docs/install.qmd) for PowerShell 7 Windows instructions,
prerequisites, manual checksum checks and PATH help. The installer comes from
the reviewed source commit; checksums detect corruption, not independent
publisher identity or reproducible builds.

## Quickstart: a small Python analysis

This example targets `0.4.0-preview.1`. Remote docs checks run it through the
downloaded release binary and the current PR code. You need Python 3 available
as `python3`; the shell commands below assume Ubuntu / WSL Ubuntu.

In a new folder, create `_proj.yml`:

```yaml
directories:
  raw-data:
    path: _raw_data
  cache:
    path: _tmp
  output:
    path: _output
build:
  scripts: [analysis.py]
```

These explicit paths match the local directory initializer's preset.
This example uses `cache` for temporary results; `raw-data` names an input
folder and `output` names a result folder, both unused by this tiny preview.
Runtime does not fill in missing directory mappings. Document rendering needs
an explicit `docs` mapping as well.

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

For another new project, `proj --project . --non-interactive init directories`
can write the three explicit directory mappings and create their folders
without Git/GitHub setup. It preserves existing configuration and does not add
a `scripts` key: an absent key allows discovery, while `scripts: []` explicitly
selects no analysis scripts.

## More detail

- [Configuration](docs/configuration.qmd) and [command reference](docs/cli/build.qmd).
- [Behavior contract](docs/behavior.qmd): intended behavior and known gaps.
- [Compatibility log](docs/compatibility.qmd): comparison with `projr`.

The existing R package is unchanged. Matching CLI, RStudio and GUI renderer
behavior has not yet been proved; new frontend integrations are separate work.
M1/M2, complete input/code/dependency provenance and analysis-free publication
retry remain unfinished. Preview recovery is bounded and does not isolate every
write or undo arbitrary external effects.

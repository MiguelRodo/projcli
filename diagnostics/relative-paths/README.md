# Unmerged relative-path diagnostic

No production code changes, merge or release are authorized for this branch.
One Linux workflow runs actual Rscript/source(), Python, Quarto document/project
and rmarkdown HTML against an unchanged installed projr pinned at
90be50132d16aee33e39c0f7f35ef906c35f5397. The released Rust binary is checksum pinned.

Fixture layout: analysis/report.qmd or report.Rmd beside root-level _raw/data.csv,
_raw/helper.R/helper.py and _raw/images/sentinel.png. Outputs map to a versioned
cache in the explicitly labeled test-only prototype. Existing projr relative/auto
getters and current Rust CLI are measured separately. The prototype reads fixture
configuration at the captured root, models concrete profile overrides, and expresses
paths relative to getwd/cwd at call time. It is not an implementation/compatibility
claim for the entire schema, frontend, version lifecycle or production build.

Native versus explicit root execution is measured per runner. R source default
chdir=FALSE and explicit chdir=TRUE are separate cases. A cwd switch to a distinct
project diagnoses whether existing helpers consume the captured-root context.
Markdown image URLs are inspected relative to output HTML, separately from code
filesystem paths. Linked/embedded images and local JS/CSS are checked before and
after HTML+stem-assets relocation and complete Quarto project-output relocation.
PDF runs only if an engine already exists, without installing TeX packages. No GUI
or RStudio parity is claimed. Individual behavior failures are findings, not hidden
by a green diagnostic infrastructure status. Read summary.json and logs/artifacts.

Fixtures receive no GitHub token/context environment or credential values. Only
known non-secret path/tool evidence is emitted. Source package is never patched.

HTML inspection happens only after copying bundles outside the original project
and renaming the entire source/cache root away. Linked sentinel images must be
inside the served output bundle and fetched over local HTTP with matching bytes;
embedded images must contain the actual sentinel PNG payload. Native-default,
explicit true and explicit false settings are separate cases, with no prototype
embedding fallback forced into renderer arguments. The proposed future fallback
is unspecified -> true, preserving explicit false/true after inherited settings.

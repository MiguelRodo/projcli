# projcli 0.4.0-preview.2

This development preview adds the CLI analysis workflow described below. The exact
tagged source commit is recorded in source-commit.txt; checksums.txt verifies both
platform downloads. These notes are prepared before publication.

Run selected R/Python analysis steps from one project configuration, inspect
temporary results, and keep the project identity stable when code changes folders.
Generated-output paths default to the selected versioned cache. Declared final
outputs are promoted only after successful production analysis; previews do not
promote or publish. Output cleanup defaults to keeping other final output files,
with explicit pre/post cleanup options. Document-output replacement follows its
separate documented rules.

Scripts use the launch folder and document tools retain their native working-folder
settings, unless a project-folder override is explicitly requested. The R-package
output-r-data role has a fixed final project/data destination; querying it never
opts that folder into cleanup or promotion. Declare it explicitly to activate
ordinary output management. Supported legacy declarations normalize before merging.

Individual Quarto/R Markdown HTML and modeled Quarto projects default to embedding
resources only when unspecified. Explicit and inherited false values remain
respected. Bookdown's site renderer is unchanged and has no new embedding fallback.
Linked-output portability and literal Markdown/LaTeX resource rewriting are not
provided. Existing R helpers/packages are unchanged; direct R/IDE parity is not
claimed.

Remote acceptance covers manual configuration and fresh local directory
initialization, ordered real Python/R/report steps, a computed CSV and embedded
report, deliberate structured failures, ordinary generated-output/archive preservation
under none and local archive publication without Git credentials. Prior document
outputs are preserved on preview/failure and replaced by successful finalization.
Final files and archived object bytes must match the computed witnesses; manifest/
index release and next-development versions are checked, with corruption probes. The reviewed release source must pass the full integrated gate, including all
26 renderer-policy cases, eight combined workflow cases and seven archive
corruption probes per successful production case.

Delivery remains checked Linux/WSL and Windows x86-64 binaries. No Rust compiler or
administrator access is needed to install. macOS/ARM binaries are not supplied.
Exact public asset checks and the user-local WSL install still precede any delivery
completion claim.

Use sample data or an independently backed-up disposable project. Recovery is
bounded and does not isolate arbitrary external script effects, concurrent writers
or power loss. Complete provenance, durable attempt/release records, analysis-free
publication retry and M2 remain unfinished.

Release checks and remaining limits:
https://miguelrodo.github.io/projcli/release-candidate.html

# Next projcli development preview — draft release notes

Unpublished preparation. The release version and exact source commit will be set
in the reviewed version change. The published/installed preview remains
0.4.0-preview.1; these notes do not announce its replacement.

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

Prepared remote acceptance covers manual configuration and fresh local directory
initialization, ordered real Python/R/report steps, a computed CSV and embedded
report, deliberate structured failures, prior final/archive preservation and local
archive publication without Git credentials. Final acceptance must rerun on the
reviewed integrated source; a stacked preparation pass does not complete M1.

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

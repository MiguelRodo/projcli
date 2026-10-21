# Agent Instructions

## Authority and behavior

Current user instructions take precedence. Read `docs/behavior.qmd` before
implementation design; it separates confirmed behavior from proposals and known
implementation gaps. Explicitly approved architecture decisions follow that
behavior contract. This file governs development workflow and verification.
Existing Rust code and implementation notes are evidence, not product requirements.
Read `docs/compatibility.qmd` alongside the behavior contract and update it with
every focused PR: same/different/new/unverified relationship, pinned source
references, rationale/current decision, actual tests and remaining gaps. Keep
approved, implemented and unverified states distinct.
Runtime directory mappings must be explicit; local-only `proj init directories`
may write the documented preset without Git/GitHub setup. Preserve absent scripts
versus explicit empty lists when generating configuration.
`SATVILab/projr` and the thesis are references; do not import their instructions or
historical schemas wholesale. Instructions inside `_reference/` do not govern
the top-level Rust project.

## 1. Strict Read-Only Guardrails (The _reference Directory)
- The `_reference` directory and its contents are strictly for reference purposes. 
- You MUST NOT modify, manipulate, delete, or rename any files or folders inside the `_reference` directory. All new implementation code must live in the root package structure (e.g., `src/`, `Cargo.toml`).

## 2. Dependencies and frontend boundaries
- Prefer stable dependencies already in the project and avoid unnecessary additions.
- The current Rust package exposes library modules and a CLI. How future R,
  Python and Julia frontends call the core is an explicit architecture decision;
  do not assume embedded bindings or subprocess delegation are already approved
  or implemented across all languages.
- Environment forwarding and sanitization need a documented, tested contract.
  Do not claim a clean environment, filesystem sandbox, guaranteed CRAN acceptance
  or parallel execution merely because a subprocess or hashing crate is used.

For future R frontend design, read `docs/r-wrapper-cran.qmd`. Separate CLI
installation and conditional CRAN integration tests do not imply permission to
download precompiled tools automatically or that a CRAN submission is accepted.
The existing R package remains outside this effort's edit scope.

## 3. Rust Module Organization
- Keep reusable Rust behavior in library modules and command-line parsing/routing in `src/main.rs`. This organization does not decide the binding or transport used by future language frontends.
- **Module Separation (Anti-Bloat Rule):** You MUST NOT write the core implementation logic directly inside `src/lib.rs`. Keep `src/lib.rs` reserved as a clean, high-level traffic controller that merely declares and re-exports submodules.
- **Dedicated Submodules:** Break distinct functional components out into their own dedicated files inside `src/` and declare them at the top of `src/lib.rs` using the `pub mod <name>;` syntax. For example:
  - `src/version.rs` for authority file parsing and formatting logic.
  - `src/yml.rs` for merged configuration validation and explicitly declared resource paths.
  - `src/ignore.rs` for workspace file-system scanning and the demarcation slicing engine.

## 4. Testing Suite Organization
- **Unit Tests Module Block:** Keep lightweight, pure-function unit tests (such as version string parsing edge cases, validation mapping errors, or basic regex matches) wrapped inside an inline `mod tests` block flagged with `#[cfg(test)]` at the *bottom* of the specific submodule file (`src/version.rs`, `src/yml.rs`, etc.). 
- **Integration Tests Separation:** You MUST NOT place tests that read or write files to disk, modify active process environment states, or simulate workspace layouts inside the `src/` folder. All filesystem-dependent or multi-module workflow tests must be written as separate integration test files inside a root-level `tests/` directory.

## 5. Documentation Strategy
- For every struct, enum, and public function implemented, you must immediately author inline Rust documentation comments (`///`).
- Include a descriptive overview, expected parameters, edge cases/errors, and an executable Rust example where useful. Rustdoc checks validate Rust examples; separately verify CLI `--help` and command examples. Neither check alone proves that API behavior, CLI help and user documentation agree.
- Run `cargo test --doc` to verify that the examples written inside your documentation comments actually compile and pass successfully.
- Verify that running `cargo doc --no-deps` generates clean, complete API reference pages without warnings.

## 6. Reference Website Architecture & Synchronization
- **Documentation Site Layout:** A Quarto-based documentation website lives in the root `docs/` directory. Whenever you introduce, modify, or extend a core framework capability or CLI subcommand, you must immediately create or update its corresponding user-facing page under this structure:
  - `README.md` – Project introduction and links.
  - `docs/behavior.qmd` – Confirmed behavior, proposals and implementation gaps.
  - `docs/flow.qmd` – Current implementation notes; do not treat historical flow as normative.
  - `docs/configuration.qmd` – Validation specifications for `_proj.yml` (prefixes, fallback rules, errors).
  - `docs/cli/` – Command usage manuals:
    - `docs/cli/version.qmd` – Reference documentation for `proj version get` options and JSON schemas.
    - `docs/cli/yml.qmd` – Reference documentation for evaluating configuration properties.
    - `docs/cli/path.qmd` – Reference documentation for path resolution and temporary build cache locations.
    - `docs/cli/ignore.qmd` – Reference documentation for `proj ignore` logic, protected lists, and boundaries.
    - *Add new `docs/cli` qmds as needed, and update their functional descriptions here.*
- Keep command reference pages aligned with implemented behavior and `--help`. Separately label approved target behavior, proposals and known gaps in the behavior contract. Do not rewrite approved behavior to match a defective implementation. Avoid blank pages or placeholders.

## Ponytail

For coding, refactoring, bug-fixing, review and implementation design, read
`.agents/skills/ponytail/SKILL.md` and apply Ponytail in **full** mode by
default. Use **ultra** only when explicitly requested. Repository-specific
correctness, security, architecture and validation requirements take precedence
over generic simplification guidance. After a completed work chunk, apply the
actual skill's full-mode review guidance and record concrete findings/actions in
the compatibility log or PR. This does not replace independent review or CI.

## GitHub issues and Projects

For issue and Project administration, use the canonical
[MiguelRodo/github-projects-skill](https://github.com/MiguelRodo/github-projects-skill/tree/main/skills/github-projects)
skill and read `.projects/project.md` for this repository's routing contract.
The central issue store owns planning; this repository is its implementation mirror.


The approved path contract separates logical safe targets, configured final roots,
path representation and runner cwd. Preserve native literal Markdown/LaTeX meaning.
Embedded HTML is the target; explicit false stays respected and linked portability,
existing R/IDE parity and M2 provenance are not new Rust M1 completion gates.
Use the behavior contract's captured root/profile/run-version rules; never infer
render context from file suffix/cwd or mutate parent environment for library runs.

Explicit safe lookup retains declaration order for eligible cache selection and must
not synthesize undeclared labels. Relative fragments cannot replace/escape the
role root. Keep final lookup separate from future safe-output promotion; switching
CLI defaults must accompany consumers, not silently strand staged production data.

Render context must be attached at the actual controlled renderer invocation,
with an optional canonical document anchor; queries never infer it from suffixes
or markers. Strict relative requests must error rather than silently change format.
Keep execution cwd, filesystem representation and literal resource URLs separate.


Native execution policy (supersedes earlier unconditional root-cwd wording):
`build` and `build-dev` default to `--execution-dir native`. Plain scripts/hooks
inherit the invocation cwd, not each source file's parent; R `source()` normally
inherits its caller cwd. Individual Rmd rendering leaves `knit_root_dir` unset and
Quarto receives no execution-dir override, preserving native document/project
settings. `--execution-dir project` explicitly requests captured-root execution
for scripts/hooks, Quarto and individual Rmd. Project render drivers run from the
selected project root to select the project, independently of chunk cwd. Project-wide
Bookdown root override is currently rejected early because its render_site entrypoint
does not expose the individual knit-root argument. Captured project/profile/version
and logical path identity do not change with execution cwd. No existing R helper
bridge or literal Markdown resource rewriting is implied.

Whenever lifecycle ordering or conditions change, update the website lifecycle
Mermaid diagram (the lifecycle page maintained alongside website navigation) in
the same reviewed work, or coordinate its owner before merge. Keep pre/build/post
groups, approved target versus shipped behavior, and event-order test evidence
aligned with docs/behavior.qmd. Do not silently overwrite concurrent site edits.


Settings are runtime policy; build selects actions, parameters are analysis data.
For output lifecycle changes retain exact clear/script adjacency and end-hook/version
adjacency. Keep the site lifecycle diagram synchronized through its owner before
merge, including the irreversible-publication boundary and preview/failure branches.
Source-only intermediate behavior is not a release or M1 completion claim.


Path defaults are safe/auto: generated output resolves into captured versioned
cache, ordinary auto is caller-relative and known-renderer auto is absolute.
Runtime HOW belongs in `settings`; execution-directory and path-representation
are merged policies with explicit CLI overrides. Keep child defaults frozen with
root/profile/version/mappings; never rediscover them after scripts mutate YAML.

HTML fallback uses embed-resources for individual Quarto documents and native
Quarto effective-format inspection and guarded temporary document-header defaults.
Respect both embed-resources and legacy self-contained presence. Restore original
source bytes on success/error/panic before promotion; reject changed prepared sources
or native input sets at launch. No global project default may override an alias.
Individual R Markdown uses self_contained only when unspecified. Preserve explicit
false, native literal resource semantics and exact source/config restoration.
Bookdown site rendering retains its native policy; do not claim that project
driver has the new embedding fallback or that nonembedded relocation is supported.

The remote combined M1 driver is tests/acceptance/m1_contract.py. Inspect its
JSON and downloadable CSV/HTML artifacts, and rerun on the final reviewed combined
source tree before release; a passing stacked preparation is not M1 completion.

R-package data lookup is built in as output-r-data with fixed final project/data;
safe lookup still requires a declared selected cache and version. Lookup never
activates writes. Only explicit canonical or per-layer normalized legacy data
declarations enter ordinary output clearing/recovery/promotion/publication.
Reject redirection and alias conflicts before merge; no special preservation
policy, new language roles or edits to the existing R package.

Embedding header defaults must retain every effective format choice and native
order. Use guarded same-directory source replacement, preserving permissions and
external hard-link bytes. Freeze relevant profile/directory/included metadata and
reject changes before rendering; never write external metadata dependencies.

Embedding inventory includes both native metadata-file/metadata-files spellings
and fixed _quarto.yml.local/_quarto.yaml.local paths, including absence. Leading
scripts changing singular includes or changing/creating local overlays must fail
before render and recover internal bytes/presence. Native actual regressions extend
the existing matrix; no custom metadata merging or external writes introduced.

Before the next preview release, follow docs/release-candidate.qmd. Keep the
published/installed version distinct from source changes; rerun exact integrated
acceptance before the reviewed version/tag/assets/publication/WSL sequence.
Unchanged Bookdown site embedding and other deferred limits must stay explicit.

Preview.2 is published from 768da1bf1b83be83b657b36219f299f37130ba03 and
verified in the WSL user-local installation. Keep README/install/public-download
expectations pinned to that source/version together. Retain the native-browser
lifecycle witness and exact delivery evidence in docs/release-delivery.qmd.
Future source edits do not update the release or installed binary automatically.
Existing R/IDE, Bookdown, linked portability and M2 limits remain explicit.


M1 completion means the released active-configuration local CLI preview, with
explicit file targets and bounded managed recovery. Keep stronger command/argv
schemas, OS-enforced confinement and broader maintenance separate; no sandbox or
M2 completion claim follows. Reuse the verified preview.2 source/tests/evidence.


Script/hook include and exclusion syntax must fail preflight on errors, including
exclusion-only selections. Never turn malformed exclusions into silent no-ops or
shell commands. Reuse the shared resolver and preserve valid first-match order,
deduplication, explicit-empty semantics and literal root handling. Source fixes
newer than preview.2 do not update the published/installed binary automatically.


Fallback discovery must propagate root/entry/type-inspection failures instead of
silently selecting no or fewer scripts. Keep its existing shallow regular-file,
no-link, language/alphabetical ordering and hidden/Git-ignored behavior. Root
enumeration during managed-state capture must retain phase and OS error context.
Permission fixtures run unprivileged and restore owned directory modes before
assertions; no new traversal, sandbox or persistence contract is implied.


For renderer recovery acceptance, retain the compound real-renderer sequence in
tests/acceptance/mixed_render_retry.py: HTML/PDF before a later document failure,
then a smaller successful Git build with exact committed/final/cache bytes and
no failed artifacts. Assert prior final state after failure, preserved unselected
native documents and unchanged retained scratch after retry. Successful final
docs replacement stays intentional. Run remotely in the pinned image; no new
manifest identity, retry namespace, sealing or backend policy is implied.


Read docs/storage-design.qmd before future storage/catalogue design. proj must
remain standalone for ordinary use; DVC is optional per directory, never the
mandatory manifest engine. Future opted-in operations delegate automatically,
with DVC authoritative for its tracking/cache and proj referencing versions.
Do not duplicate DVC's authoritative state or silently install it. Preserve the
curated linear main-branch release-catalogue direction without deciding attempt
identity, sealing, schema, persistence or provider contracts. This is a future
integration constraint, not authorization to implement M2 persistence.

Backend choice must preserve proj's logical directory labels/roles and path
abstraction; analysis code must not require DVC-specific physical paths. DVC's
physical tracking distinction does not imply an absence of named stages/config.


Inventory groundwork must test existing explicit CAS snapshots against known
path/content fixtures, including changed bytes, additions, renames and deletions.
Preserve previous evidence across ordinary ingestions. Do not infer a future
build-record schema, rename-event format, content-only identity, retention policy
or lifecycle capture point from this existing-format acceptance coverage.

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
  Python and Julia frontends call the core is an explicit architecture decision.
  M4 explicitly selects a thin R subprocess adapter in frontends/r/projcli;
  do not infer embedded bindings or approved integration for other languages.
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
do not update the published/installed binary automatically; delivery requires
reviewed release and installation verification.


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


Keep optional-Git integration evidence in tests/test_real_git_publication.rs:
actual local repository/bare upstream, non-main/differently named tracking ref,
explicit push settings, real no-op provider commits and rejected-push versus
analysis-failure outcomes. Compare complete upstream branch refs and actual
committed/final/cache bytes. Restrict fixture transports to local files and use
per-command isolated Git configuration; never mutate parent environment. This
does not settle sealing, persistence or remote hosting/provider contracts.

The real-Git no-op provider must run in a fixture-controlled subprocess because
SystemGit::new inherits the process environment. Clear inherited GIT_* overrides
for every fixture command before setting its scoped config/file-only transport.
Keep the hostile-inheritance victim-tree witness; do not call set_var/remove_var
globally from parallel tests or introduce production APIs solely for test setup.


Inventory groundwork must test existing explicit CAS snapshots against known
path/content fixtures, including changed bytes, additions, renames and deletions.
Preserve previous evidence across ordinary ingestions. Do not infer a future
build-record schema, rename-event format, content-only identity, retention policy
or lifecycle capture point from this existing-format acceptance coverage.

## Temporary CSV inventory scope (23 October 2026)

User-approved interim manifest.csv follows projr label/fn/version/hash, MD5 bytes
and production pre-input/post-output/history semantics. Keep syntax in
manifest_csv and capture/history in manifest; CSV is independent of CAS.
Preserve guarded writes, absent-file rollback and existing lifecycle. No mandatory
cache for cache-free builds. Directory hash defaults true except cache; reject
non-booleans before layer merging. CAS is a third remote structure alongside
latest/archive; full store/restore, retention/discovery and identity/sealing are
separate. Exact-head remote tests and independent review are required; evidence is recorded on PR175.

## Remote parity implementation boundary

Use docs/remote-compatibility.qmd as the pinned source inventory, not a claim of
implemented parity. Port working local/GitHub latest/archive options in bounded
reviewed slices; distinguish accepted-only fields and source bugs. Preserve
path guards and honest publication failures. CAS is additional; no retention/GC
or discovery policy follows. The future registry proposal remains issue-only.
Never use user datasets or destructive live-provider operations for fixtures;
remote heavy tests and mocked/disposable transports are the default.

Ordinary local version inspection is read-only and independent of CAS/config
registry changes. Distinguish advertised latest metadata from present directories
and verified bytes; never turn an untrusted-marker absence into a trust guarantee.
Archive numeric ordering must not be lexical or assume exactly four components.
Retain hard-link/symlink/special-file guards and no-mutation fixtures.

Ordinary local download uses explicit version/source and destination arguments;
do not silently adopt the future registry design or configured restore fallback.
Stage all copies before replacing an existing destination; default preserves
destination-only files, explicit clear discards them only after staging succeeds.
Retain original directories if rollback fails and report their path. Preserve
hard-linked aliases and reject linked/special copied trees. Do not claim CSV
integrity verification, crash atomicity or concurrent-writer isolation.

Explicit remote store is a file-inspected physical-directory policy layer.
Require explicit file inspection, preserve sync-diff strategy semantics and
dry-run no-mutation. Do not imply VERSION/CSV publication, configured cues,
historical trust or future registry decisions. Share inventory/guarded install
logic with existing build/download and retain their failure/permission tests.
Mirror plans describe file actions, not private staging I/O counts.

Native GitHub Release asset transport must omit tokens/signed URLs from its own errors and never
forward Authorization on download redirects. Keep explicit new-upload duplicate
rejection and staged bounded downloads; no deletion/overwrite, ZIP/configured
restore or credential-helper parity claim follows from the initial transport.
Use mocked HTTP fixtures and remote tests, never live user dataset uploads.
Document that embedders must not enable unredacted ureq DEBUG/TRACE logging;
no network endpoint confinement follows from stripping redirect credentials.

GitHub ZIPs use projr's relative members, flat latest/archive names and shared
manifest/VERSION/CHANGELOG ZIP names. Extract only to private temporary trees,
bound compressed and expanded bytes independently, validate CRC/portable paths
and reject links, aliases, duplicates and special entries before guarded install.
Empty variants require only the zero-byte projr-empty sentinel. Explicit directory
transfer is not configured restore, metadata history merge or overwrite parity.

GitHub replacement is explicit --replace. Stage complete local bytes and validate
the unique asset/upload endpoint before DELETE; preserve strict new-upload default.
DELETE followed by upload is the pinned projr behavior, not atomic publication:
report deleted/uncertain outcomes and never silently retry or imply rollback.
Mock fixtures only; no existing user assets are test targets.

Configured restore uses existing directories.LABEL.source and build provider
definitions; issue706 registry remains design-only. Keep source-before-destination,
preferences/exclusion, merged profile order and numeric/advertised version rules.
Never clear before source lookup/staging; empty/unrecorded labels skip intact.
Restore's config projection must not make normal builds silently accept ignored
ordinary upload fields. Correct source indexing/inline bugs with explicit evidence;
do not add implicit archive paths, new policy defaults or CSV trust claims.

Configured ordinary publication must fail namespace preflight before hooks or any
destructive replacement, including full/empty/metadata aliases and selected CAS
overlap. The bounded first slice accepts local latest with explicit file
inspection only; never silently downgrade the existing manifest default.
Freeze plans before hooks, stage current CSV-selected files for every target
before applying any target, preserve directory modes and explicitly clean all
private staging on every ordinary Result path. Retain the release on publication
failure and report partial effects. Keep the tested/pending control matrix honest.


Canonical output-r-data publication must reject legacy data/data-empty entries
and data VERSION advertisements before hooks and again during all-target
preparation. Preserve legacy restore; do not silently migrate or delete history.


No-inspection publication changes decisions, not safety guards. Preserve projr's
none/default-upload-all interaction; explicit missing/diff blindly upload source
files and keep extras, while empty-source purge does not infer remote extras.
Do not advertise none as zero filesystem reads/hashing. Keep manifest default
separate; retain the actual build/store/restore and empty tests.


Default local latest manifest inspection compares matching recorded project and
remote label/version rows only for unstarred metadata; otherwise use file fallback.
Never strip the untrusted marker to establish trust. Preserve actual unselected
bytes under trusted partial sync-diff, full replacement under purge, and guarded
physical staging. Malformed history still fails closed; no authenticity claim.


Ordinary local archive publication defaults to archive and preserves all older
versions. Freeze eligible historical comparisons during all-target preparation;
require continuous unchanged history and catalog/VERSION/CSV agreement for reuse.
A new version is always complete, never only a diff against an older directory.
Keep empty variants, shared metadata and exact-version alias preflight bounded;
configured append-label:false is still rejected. Update tested/pending matrices.


Configured GitHub publication supports latest/archive + file/manifest/none inspection and all four strategies.
Freeze Git/API/token/tag before hooks; stage all CSV-selected restorable ZIPs
before uploads. Preserve history/data-before-metadata ordering. Merge canonical
current data through guarded staging, retaining remote extras. Reject portable/
numeric aliases, simultaneous full/empty assets and legacy ambiguity. Validate
empty sentinels even when the source is empty; leave existing data/label VERSION
unchanged on empty upload-all. Mark updated uninspected targets untrusted. Remove
obsolete empty variants only after successful full upload. Report partial metadata
replacement; mock REST is not live GitHub evidence.


Non-public API origins require an explicit environment token. Default github.com
Git/gh credential helpers are only used for the standard HTTPS api.github.com
origin (port443), never forwarded to Enterprise/custom hosts. A regression uses
a fake public helper and verifies it is not invoked for a loopback API.


Public gh credential fallback explicitly requests --hostname github.com, so an
inherited GH_HOST cannot select Enterprise credentials for the public API.
Numeric archive alias checks remove exactly one generated .zip suffix, preserving
valid prefixes/labels that themselves end in .zip. Regression fixtures cover both.


GitHub none inspection follows the existing local interaction: missing/diff upload
all selected files and retain extras; nonempty purge replaces the complete set.
Empty sources preserve data; only diff advances untrusted label VERSION on that
no-op. Purge nonempty is unstarred. Keep actual CLI mocked layout/strategy/cue
matrix separate from live provider evidence.


Configured GitHub comparisons reuse the ordinary local publisher on caller-owned
private mirrors, with shared CSV trust/history eligibility and returned current
data effects. Materialize only actual current data and the eligible newest archive;
catalog-only placeholders must be rejected by the same history eligibility before
their bytes could be inspected. Keep full new snapshots, metadata-only no-ops,
empty variants, all-target preparation and truthful DELETE/POST failures intact.
Defaults remain archive/manifest/sync-diff/if-change; registry706 stays design-only.


The real GitHub witness is docs/github-live-evidence.json and the remote
compatibility page: source03074c8, exact-main CI artifact/binary hashes, five
synthetic builds/two restores in a dedicated prerelease. Preserve the distinction
between that witness source binary (whose package string was preview.2) and
product releases. Published/installed preview.3 packages the subsequent reviewed
source95441429; its delivery evidence is separate from the synthetic fixture. Do not describe mocked full control coverage as live coverage.
Keep fixture data distinct from product releases and never use user assets for
live tests or replay an uncertain mutation without readback.


Real-Git failure witnesses must retain fixture-local configuration/credential
isolation. Distinguish missing-upstream preflight from commit-hook rejection
after successful analysis. A failed commit can leave the new local release staged
in Git's index while HEAD/upstream stay unchanged; never assert an invented index
rollback, successful publication or new sealing policy from that condition.

Build ownership uses one cooperative .proj/build.lock claim for the entire
build/preview function, before input capture and through recovery/finalization.
Never steal an existing claim or treat its timestamp as proof its owner died.
Protect .proj from managed cleanup/publication destinations. The live claim is
excluded from Git staging/status by an exact reserved-name pathspec at every repository depth without
narrowing the existing whole-repository scope; successful/distributed-failure logs must not be hidden.
The ownership guard is separate from publication-attempt evidence below;
crash recovery remains separate work; approved local-only failure diagnostics are described below. Preserve existing hook semantics; mutation enforcement
is deferred, not an M2 gate.


Pre-distribution failure recovery preserves exact prior VERSION/DESCRIPTION
bytes and absence, including development counters above one and accepted original
formatting. Never synthesize dev1 as rollback. Publication-boundary behavior and
end-hook lifecycle remain unchanged; hook-mutation enforcement is deferred.


Publication-enabled production builds retain allowlisted JSON evidence in
.proj/log. Keep the pre-Git snapshot immutable once sealed; write the final Git
and finalization outcome to a new receipt for the next ordinary commit. Never
create recursive bookkeeping commits or infer remote rollback from a failed
request. Mark possible activity durably before provider/Git entry. Preserve
both primary-operation and evidence-writing errors; retain the earlier record.

Only exact untracked receipts with an unchanged committed snapshot and a matching
immutable worktree-local Git metadata witness may bypass the clean-state guard.
The witness binds project identity, attempt and receipt bytes; it is control
state, not a catalogue or authentication against users editing Git metadata.
Do not hide modified/tracked receipts, arbitrary neighbouring files or unowned
VERSION edits. Keep snapshots/receipts under normal Git staging; never stage the
claim or ownership witness. Do not serialize raw environment, credentials,
remote URLs, command arguments, descriptions or subprocess output.
Successful plain production records and approved local-only production failures
are covered below. Previews remain unchanged; retain bounded provenance claims.


Successful plain production builds retain completed attempt evidence after all
recoverable work succeeds. Failed production attempts follow the approved local-only
policy below; do not introduce preview records implicitly. Bind current input/release-output inventory evidence
to the existing canonical CSV codec; keep large per-file rows out of JSON.
Observe final outputs separately and preserve the immutable pre-Git snapshot.
Output mutation by hooks remains allowed, not an M2 enforcement gate. Fingerprints
cannot recover missing CSV or enumerate changed final rows; keep these limitations
and the remaining full provenance/human-history work explicit. New optional
record fields must still decode earlier pending development receipts.


Startup provenance is an explicit observation before start hooks, not proof of an
immutable execution tree. Reuse preflight runner responses without extra probes;
retain only recognised numeric versions, not vendor output/paths. The probe cap
bounds retained metadata, not existing subprocess buffering. Never serialize raw
configuration, environment, remote locations or arbitrary parameters. The policy
projection records validated flags/counts only. Resolve physical identities before
classifying relative-parent sources as internal; redact external/overlong names.
Fingerprint only guarded regular files within the source/count/byte limits, with
explicit unknown/limit states. Root dependency declarations are not installed
package fingerprints. Do not infer full environment reproducibility or select
failure-retention policy from this bounded companion evidence.


## Final rows and derived history
proj history is read-only and independent of Git/active configuration. Reuse
attempt records and existing four-column CSV; final observations have bounded
checksum-bound per-attempt CSV companions, not a new catalogue or manifest.
Git pending-evidence exceptions require the intact receipt/CSV bundle and existing
ownership witness; clone history reading does not require that private witness.
Keep old fingerprint-only evidence unavailable unless exact historical rows match.
Never turn missing terminal receipts into successful outcomes or overwrite the
curated changelog. Preserve failure retention and snapshot/receipt ordering.


## Effective configuration and M2 gate
Fingerprint the already merged alias-normalized analysis subset before it is
consumed or changed, then bind effective CLI/merged policy. Canonical maps sort
keys; arrays and directory declaration order stay ordered. Use typed redactions
before hashing and expose omissions/limits; never retain raw parameters/URLs.
Keep bounded-out evidence nonfatal with no prefix digest. The recognizer is not
universal secret detection. Preserve CSV, hooks, receipt timing and the approved
local-only pre-distribution failure policy below. Run tests/acceptance/m2_contract.py plus its independent Rust
byte witness remotely alongside the existing pinned matrix. Assess readiness
against actual M2 criteria; do not add full environment/transitive freezing gates.


## Local pre-distribution failure diagnostics
Production failures after acquiring writer ownership now retain a bounded,
allowlisted diagnostic under .proj/log/local-failures/ after ordinary recovery.
The directory's own exact '*' ignore policy and framework Git pathspecs exclude
only these local diagnostics, including at nested project depths. Reject
force-tracked claims or diagnostics rather than hiding tracked changes.
Successful records and failures after possible distribution remain normal Git
evidence. Never serialize raw errors, argv, parameters, URLs or personal identity.
Version observation must reject linked, special and oversized files. Keep one
record per attempt when publication preparation fails without provider entry.
History distinguishes local failed attempts from successful releases. Do not
invent pruning, crash recovery, preview retention or new hook semantics.

Hold managed recovery until provider/Git action entry, including failure to save
pre-entry evidence. Saved intent/configured empty targets are not action entry.
Pre-entry preparation/end-hook/observation/evidence failures restore exact prior
managed state before local diagnostics observe after-version. After entry,
preserve local release/partial-distribution evidence and never fabricate rollback.


## Versioned M3 metadata codecs
snapshot_format is a pure library codec slice. Preserve explicit schema strings,
BLAKE3 derive-key contexts and canonical ordering once reviewed; pretty bytes are
not IDs. Directory roots exclude label/version/date; snapshot metadata is separate.
No implicit clock, legacy ID reinterpretation, destructive conversion, delta-only
flat view or size-savings claim. Complete flat views include empty directory leaves.
Bound derived prefix work before allocation as well as input bytes/records/depth.
This module alone does not enable registry/placement, filesystem publication or
restore. Reuse guarded immutable helpers and verify typed graph objects before
future consumers follow refs; retain ordinary CSV and existing flat CAS readers.


## Local v1 tree-store APIs
cas_tree explicitly separates file-content and metadata roots. Reuse raw CAS
objects; new trees and snapshot refs are first-two/rest hash-sharded and use
the reviewed v1 canonical identities. Legacy flat manifests/ledger stay unchanged.
Guard planned authority/control aliases before writes; use the same native
blob-prefix validation on publication and reading. Pre-scan source links/special
entries/names before writes, recheck on open, and hash the copied stream.
Snapshot refs publish only after full graph identity/file-byte verification.
Bound unique metadata, recursion and expanded DAG paths; no local index is
required for explicit remote snapshot reading. Preserve semantically identical
immutable JSON variants without overwriting them. Failure can leave unreferenced
objects; no GC/crash-atomic/multi-location transaction claim.
The APIs do not yet activate registry/build policy, Git refs, restore or CSV/JSON
catalogue deduplication. Latest flat serialization work keeps both CSV and JSON,
with per-artefact VERSION-level references by default and optional file-dedupe
modes; the complete JSON codec remains an explicit full-inventory/export view,
not a JSON-only configuration default. Do not alter flat policy pending its schema.


## Catalogue codec boundary

Preserve four independent choices: provider/layout, manifest placement,
flat/tree structure and CSV/JSON serialization/dedupe mode. New catalogue v1
records may directly reference preceding complete same-label inventories, never
chains. Dictionary modes must retain every current path and bundle their full
hash dictionary. CSV repeats label/version on membership rows for filtering and
uses explicit record_index to survive row sorting. Do not silently replace the
ordinary projr CSV or old CAS readers, claim CSV references auto-expand in Excel,
or promote codec-only groundwork to active remote support.


## CAS materialisation boundary

Reuse the ordinary guarded installer; CAS adds private-stage byte/membership
verification before replacement. Explicit clear=false preserves destination-only
entries; clear=true requires exact selected membership. Never restore into CAS
namespaces or project authority/control paths, mutate file aliases in place,
require the local ledger for explicit remote recovery, or claim crash atomicity,
permissions restoration or active CLI/config wiring from the library alone.


## Ordinary remote registry boundary

Registry definitions are source lookups independent of build upload selection.
Mapping build.dest selects explicit names and optional content subsets; never
upload all registered definitions implicitly. Preserve legacy inline/CAS-list
execution and source priority. Normal directory-source annotations validate
references but do not activate automatic build-time restore. Reuse existing
ordinary provider validators/engines. Reject unsupported placement/style/format
fields until implemented; do not accept them as ignored options or call the
ordinary registry slice complete tree-aware M3 integration.


Modern configured local CAS publication keeps file objects in the store, full
metadata per placement and a small project reference last. Retain the separate
legacy content/inspect parser and old IDs/ledger; never migrate in place. Prepare
sources before actual provider entry. Once entry is recorded, preserve release
and honest partial effects. No atomic both-copy/GC/discovery claim follows.
Flat catalogue options and configured recovery remain separate reviewed slices.


Explicit CAS snapshot commands use an exact named local store and stored ID. Upload
selection/style must not hide older explicit snapshots. Check all present full
metadata copies and actual bytes; never substitute a small root reference or
silently reinterpret a legacy flat ID. No CSV, VERSION, log or remote writes on
verification/recovery. Keep automatic snapshot selection and flat runtime work
separate, and ordinary restore selection unchanged.

Legacy CAS recovery preserves raw JSON IDs and the original namespace. Never
relabel them as tree roots. Reuse bounded reads, portable path checks and the
shared private installer; do not invent empty-directory history. Format selection
must reject ambiguous same-ID namespace presence and never write during verify.

Local CAS flat runtime must preserve ordinary root MD5 CSV and legacy/tree IDs.
Keep style, serialization/dictionary mode, placement and raw object dedup distinct.
Only decoded compatible history prefixes may merge; prior label/version inventory
is immutable. Existing store Attempt claim protects authoritative history reads
through atomic replacements and refs. Unknown claims fail closed; no auto cleanup.
Required histories then full refs then project binding. No cross-location rollback,
history truncation, GC, unnecessary tree-object intermediate or performance claim.
Other provider formatting remains explicitly pending until implemented/tested.


Ordinary local flat histories observe actual final payloads, never source CSV
substitutes. Build version and physical archive location are distinct; preserve
legacy VERSION* trust markers and latest overwrite limits. New registry defaults
must not silently opt legacy inline builds into metadata. Guard final private-stage
same-version conflicts before replacement, and reject overlapping metadata namespaces.
Ordinary data stays outside CAS objects; GitHub/tree integration remains separate.


GitHub flat histories must observe exact prepared ZIPs or explicitly materialized
reused archives, never catalogue placeholders or ZIP-omitted empty folders.
Reserve new metadata assets, validate allowed members/per-file/aggregate/member
limits, and round-trip generated bundles through the native reader. Build catalogue
indexes once per bundle. Stage records before HTTP; all required target HTTP and
project full metadata writes precede small project references. Replace new history
only against its originally observed asset ID; no automatic retry/lock/transaction.


Configured flat CAS must read/guard both fixed known metadata histories while
writing only selected full copies. Reject divergence before hooks and recheck
under the store claim. Preserve explicit-bases public API scope and existing IDs;
placement continuity is not discovery or automatic migration.


Ordinary local tree style keeps data in ordinary folders and uses the existing
DirectoryTree v1 identity, with a readable Catalogue companion excluded from pure
root identity. Charge inferred keys/nodes/prefixes globally before allocation.
Validate final-private-stage representability before replacement; cross-check
graph membership/projection and validate every node ID before traversal. Full
objects/history/bindings precede references. Keep flat bytes/IDs and ordinary
restore unchanged; never route ordinary file hashes through CAS objects.
GitHub tree writes reuse this graph codec with bounded allowlisted bundle reads,
a separate binding domain and disk-backed preparation. Check graph/projection
agreement before any HTTP write. All required HTTP and full project copies must
precede small references; defaults remain flat and old flat IDs stay unchanged.


The integrated M3 remote gate builds with ordinary tree and native CAS targets,
recovers explicit snapshots into a fresh configured project without old indexes,
and checks corruption failures preserve destinations/store bytes. Retain its
artifacts and independent Rust byte witness; automatic discovery/performance and
permission restoration remain outside this gate.


Legacy CAS ingestion must coordinate both its physical object store and the
shared project .projr lookup ledger, including public API calls to different
stores. Use distinct ledger ownership rather than reacquiring the build's project
claim. Preserve raw JSON IDs and index bytes; skip only exact version/label/digest
repeats, retaining differing digests and historical opaque rows. Do not silently
turn legacy same-version content changes into a new conflict policy.


## M4 R transport boundary
The separate frontends/r/projcli package calls a separately installed native CLI
through processx argument vectors and protocol/capability negotiation. Keep workflow,
paths, rendering and recovery in Rust; never copy or edit SATVILab/projr. The initial
transport is not a drop-in legacy API, backend switch, dependency restorer or M4 gate.
Forward active library paths through R_LIBS, baseline PATH/HOME/locale, and only
explicit additional variables. Never inherit the whole R session environment or
serialize raw argv, credentials or child diagnostics into error conditions. processx
adds required Windows/control variables; startup files and arbitrary scripts still
have their normal effects. No clean-room, sandbox, parallel-speed or CRAN-acceptance
claim follows. Package setup/checks never download proj or install R dependencies.
Keep unconditional validation/mock checks and required Linux/Windows native-executable
integration separate. Our CI pins and checksum-verifies preview.4 for transport
compatibility; it does not test new Rust behavior or the complete M4 representative
reanalysis. Use remote tests, review the exact final head, and keep M4 PRs draft
until approved; no M4 merge/release/install authority is inferred from M3 approval.

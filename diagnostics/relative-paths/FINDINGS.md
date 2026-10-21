# Relative-path diagnostic findings — 2026-10-21

Diagnostic only; no production changes, merges or new releases. Tested harness
commit `d8231dee724a9a5796e92e0f1ff4b67607926073`,
[completed remote Linux run](https://github.com/MiguelRodo/projcli/actions/runs/39554636201).
Download its `relative-path-diagnostics` artifact for summary.json, individual
path/image records, renderer return codes/logs and source-hidden relocated bundles.
The workflow succeeds only with all required evidence present; behavior failures
are recorded findings, not infrastructure failures.

## Actual execution

R 4.6.1; projr 0.5.2.10 pinned unchanged at
`90be50132d16aee33e39c0f7f35ef906c35f5397`; rmarkdown 2.32; knitr 1.52;
Quarto 1.10.19; Pandoc 3.10; Python 3.12.15;
checksum-pinned released Rust CLI 0.4.0-preview.1.

35 cases (including explicit untested-PDF record), 36 path-evidence files,
24 real HTML renders, zero renderer process failures, 48 successful code-image
lookup records, no missing/invalid required evidence. Four of 24 HTML+stem-assets
copies retained all three images; five of ten whole Quarto-project output copies
did. Twenty incomplete HTML+stem copies are substantive findings, not 20 failed
render commands. Every relocation check hid the entire original source/cache
tree and served the copied bundle over local HTTP; sentinel PNG bytes were hashed.

## Filesystem behavior

* Rscript/Python inherit launching cwd. The harness separately launched from root
  and analysis; it does not establish an automatic script-parent cwd rule.
  R source(chdir=FALSE) retained caller cwd; chdir=TRUE used the sourced file's
  directory. Native Rmd/Quarto document execution used analysis; explicit root
  execution used the project root.
* From analysis, unchanged projr relative and auto calls correctly read
  `../_raw/data.csv`, sourced `../_raw/helper.R`, and found the image. Independent
  output lookup failed because it tried to read bare `VERSION` from execution cwd.
  Profiles and absolute configured paths retained successful input stages.
* After setwd to a different project, unchanged projr read that project's sentinel
  (91 instead of 17), despite PROJ_PROJECT_ROOT. This confirms root rediscovery;
  unchanged R does not consume the Rust root variable. Helper/image content in
  both projects was identical, so those stages do not independently prove pinning.
* Released Rust remained pinned to the original root but returned absolute
  configured `_published/output`, without safe cache/version remapping.
* The labelled prototype read the captured root's fixture config/profile and
  expressed targets relative to actual getwd/cwd. It read data, sourced helpers,
  found images and wrote cache output from nested and changed directories.
  Example from analysis: `../_cache/projr/v1.2.3.4/output/...`.
  This literal fixture run-version is not a lifecycle implementation: unchanged R
  normalizes the same VERSION bytes to `v1.2.3-4`. The prototype does not establish
  version-format compatibility, complete schema merging or frontend integration.
* Python cases successfully performed their reads/helper/image/write with both
  released Rust and prototype. Their evidence is combined, unlike R's independent
  role failure capture; no Python failure-stage independence is claimed.

## Renderer/resource behavior

Each report contained native Markdown, unchanged-projr include_graphics and
prototype-relative include_graphics sentinel images. All 48 code lookups found
the file. That does not mean the generated URLs were correct.

* Native Rmd default/self_contained:true embedded all three exact PNG payloads.
  Explicit false and sidecar-inherited false remained linked; HTML+stem copying
  lost the sibling `_raw` images (HTTP404).
* Native standalone Quarto default/explicit false remained linked and lost sibling
  images after HTML+stem copying. Explicit embed-resources:true embedded all three.
* Native Quarto project default/false/project-inherited false/profile-inherited
  false retained all three linked images when its whole output tree was moved.
  HTML+stem-only copying lost them. Explicit true embedded all three.
* With root execution, both code helpers returned `_raw/images/sentinel.png`,
  valid relative to execution cwd, but those URLs were consumed relative to the
  nested document/output location. Both code-image resources failed, including
  explicit embedding; renderers warned yet returned zero. Native Markdown's
  document-relative `../_raw/...` was a separate successful resource case.
* The proposed fallback unspecified->true was NOT applied in these diagnostics.
  Existing defaults and explicit/inherited settings were measured separately.

| Case | HTML + stem assets: all 3 images | Whole project: all 3 images |
|---|---|---|
| rmarkdown-native-native-default | yes | not applicable |
| rmarkdown-native-linked | no | not applicable |
| rmarkdown-native-embedded | yes | not applicable |
| rmarkdown-native-inherited-false | no | not applicable |
| rmarkdown-root-native-default | no | not applicable |
| rmarkdown-root-linked | no | not applicable |
| rmarkdown-root-embedded | no | not applicable |
| rmarkdown-root-inherited-false | no | not applicable |
| quarto-document-native-native-default | no | not applicable |
| quarto-document-native-linked | no | not applicable |
| quarto-document-native-embedded | yes | not applicable |
| quarto-document-root-native-default | no | not applicable |
| quarto-document-root-linked | no | not applicable |
| quarto-document-root-embedded | no | not applicable |
| quarto-project-native-native-default | no | yes |
| quarto-project-native-linked | no | yes |
| quarto-project-native-embedded | yes | yes |
| quarto-project-native-inherited-false | no | yes |
| quarto-project-native-profile-false | no | yes |
| quarto-project-root-native-default | no | no |
| quarto-project-root-linked | no | no |
| quarto-project-root-embedded | no | no |
| quarto-project-root-inherited-false | no | no |
| quarto-project-root-profile-false | no | no |

## Current Rust contract, source inspection only

Pinned source: `2e706f5c9f161fe10ac22de17d699f4c357b7d1d`; src/tests identical
to released `bbd12edc8de9865a96b6145d0c7702f1300cb1a8`.

| Concern | Current behavior | Source |
|---|---|---|
| Outside-build path query | Configured absolute target; relative mappings join captured root; no safe/run-mode/caller-relative presentation | src/yml.rs499–513; src/main.rs264–268; src/machine.rs65–68 |
| Configured roles | Explicit raw/cache/output/docs-class labels; no generic project/code/data-role synthesis | src/yml.rs341–399 |
| Dev/production output queries | Same configured final output target in both; no logical cache-backed output context | src/yml.rs499–513; src/build.rs72–100 |
| Renderer project staging | cache/projr/<prebuild-version>/docs, without leading v; this is not a general logical output resolver | src/build.rs379–381 |
| Individual documents | Render beside source; promote expected file plus stem_files into configured docs | src/build.rs947–988 |
| Other scripts | No logical output redirection or general cache-output promotion | src/build.rs1083 onward |
| Version | Child queries see old VERSION; production writes incremented VERSION only after hooks. Example1.2.3 patch: staging1.2.3, eventualVERSION1.2.4 | src/build.rs214–241,432–436 |
| Profiles | Parent YAML initially uses selected PROJR_PROFILE; later build.profile can change child PROJR_PROFILE. QUARTO_PROFILE participates in env-file selection, not identically in YAML merging | src/build.rs72–120; src/run_environment.rs17–65; src/yml.rs602–630 |
| Parameters | Parsed/merged and getter exists, but no execution transport/query consumer found | src/yml.rs666; source-wide caller search |
| Nested child CLI/cwd changes | Pinned root env wins, child retains actual caller cwd; path query re-reads live config and returns absolute final target | src/ignore.rs263–272; src/machine.rs96–98 |
| Execution cwd | Current scripts/hooks/renderers forced to project root; this is superseded user policy pending correction | src/build.rs1083 onward |
| Restore | Captured metadata/config/root files, selected sources, outputs/docs, renderer intermediates; preview also local stores/VCS, on success and failure. Production failure restores captured state; publication failure keeps local success. Cache excluded | src/build.rs275–340; src/build_state.rs |

Existing tests establish current implementation, not desired path parity:
test_project_context.rs44–68 explicitly expects root/_output after nested cwd
change; test_root_renderer_contract.rs22 asserts now-superseded forced-root cwd;
test_run_environment covers propagation; preview_safety/build_restoration cover
bounded recovery. Fake-render fixtures do not establish real resource portability.

## Smallest proposed correction — awaiting contract review

1. Separate configured final resource roots from logical run-role targets and
   caller-relative presentation. Capture root/effective merged config/profiles/
   canonical run version once; child queries consume that context after cwd changes.
   Do not globally reinterpret get_path where guards/init/ignore/CAS need final roots.
2. Map safe output roles to cache/projr/v<effective-version>/<exact-label>; keep
   final destinations explicit. Preserve inspectable preview cache output.
3. Restore native per-runner cwd behavior with explicit root toggle; do not assume
   R source/Python/Rscript have the same defaults as Quarto.
4. Treat include_graphics/resource URLs as a renderer adapter concern. Normalize
   resolved image/resource targets for the renderer's document base or stage a
   complete linked bundle; filesystem-relative return values alone cannot solve it.
   Default HTML embedding only when effective setting is unspecified; preserve
   explicit/inherited false and move complete project bundles when linked.
5. Retain defensive guards and recovery while rewiring output consumers. Temporary
   VERSION/config/context and failed controlled promotion require restoration;
   broad source/output snapshots are incidental defense, not the logical path model.
   Reassess their scope only after controlled cache-backed execution is verified.

No production edits yet. Existing R package and thesis remain untouched. No PDF
engine existed; PDF image validation, GUI/RStudio, Windows/macOS real rendering,
full projr_build/version lifecycle and final frontend bridge remain untested.

Ponytail full review used the repository's actual SKILL.md. Reused existing native
render switches and Python stdlib HTML/HTTP/hash checks; no new production resolver,
framework, wrapper, dependency architecture or TeX installation. Fixed shared
evidence capture so output failure cannot hide R input results, required explicit
image records and recognized actual Quarto accessibility markup. Independent
review/CI remain separate. Original checkout's four pre-existing dirty paths intact.


## Six-case renderer-adapter follow-up

[Completed remote run39555352442](https://github.com/MiguelRodo/projcli/actions/runs/39555352442),
tested harness `032572c`, same exact tool/package versions as above. Download
adapter-summary.json, six logs and source-hidden output bundles from its artifact.
All six renders returned0; all six validated the execution-relative image target
and actually read CSV value17 before calling include_graphics. Five of six
relocated HTML files contained the exact embedded PNG payload. This run used
canonical fixture VERSION v1.2.3-4; it did not reinterpret the earlier version
spelling mismatch as a cache escape.

Root execution, explicit embedded HTML, nested analysis/report document:

| Adapter argument/control | Rmd sentinel embedded after relocation | Quarto sentinel embedded after relocation |
|---|---|---|
| Absolute target, default include_graphics options | yes | yes |
| Absolute target, rel_path=FALSE | yes | **no: rewritten local URL, HTTP404** |
| Document-relative resource URL, rel_path=FALSE,error=FALSE after validating absolute file | yes | yes |

The ordinary helper still returned `_raw/images/sentinel.png` relative to root
execution cwd; data reads were not changed to document-relative lookup. Only the
renderer adapter converted that validated target. Normal include_graphics handling
of an absolute image is the smallest common successful candidate in these two
embedded-HTML tests. Do not disable relative conversion globally: Quarto1.10.19
rewrote the raw absolute path in that variant into a missing `./home/runner/...`
resource even though rendering returned0. Document-relative resource mapping was
also successful, but disabling knitr's cwd-based existence check is justified here
only because the exact absolute target was already validated.

This is measured output behavior, not a claim about undocumented internal
normalization. The [knitr manual](https://pkg.yihui.org/knitr/manual) documents
rel_path and error; it does not guarantee raw absolute URLs work in every renderer.
No linked adapter, Quarto project adapter, PDF, GUI/RStudio, quarto R API or final
frontend integration was exercised in this follow-up. Preserve those limits before
choosing a general adapter or changing production. No further implementation or
release was performed.


## Proposed auto representation API — contract review required

Status: proposal only, not implemented or approved API spelling. User narrowed
current image support to embedded output. Explicit embedding false must remain
respected; linked-output portability/repair is deferred and may remain broken.
No further bundle-repair diagnostics are proposed here.

Keep three operations separate:

1. Capture one run context: canonical project root, ordered effective profiles,
   validated effective directory mappings, canonical effective run version and
   preview/release mode. Resolve logical role targets from that snapshot, including
   safe cache/projr/v<run-version>/<exact-label> mapping where applicable. Configured
   final destinations remain distinct for guards/promotion/storage consumers.
2. Choose execution directory independently: runner-native behavior by default,
   explicit project-root execution toggle. This never changes role target identity.
3. Present the resolved target according to the requested representation:

| Representation | Result |
|---|---|
| auto, ordinary script/query | Relative to actual caller cwd at lookup time |
| auto, explicitly known document-render context | Absolute filesystem target |
| absolute | Absolute filesystem target in either context |
| relative | Relative to explicit base; caller cwd if omitted |

Possible CLI extension, not shipped:

```text
proj path get --label raw-data --format auto
proj path get --label raw-data --format absolute
proj path get --label raw-data --format relative --relative-to cwd
proj path get --label raw-data --format relative --relative-to project
proj path get --label raw-data --format relative --relative-to document
proj path get --label raw-data --format relative --relative-to /absolute/base
```

`--relative-to` requires relative representation; reject contradictory options.
The document base requires an explicit document descriptor; never infer it from
cwd, a filename suffix or an unrelated Quarto installation. Custom bases are
absolute directories, avoiding ambiguous resolution of the base itself. Explicit
relative/absolute overrides win over auto. Same-volume relative conversion must
resolve back to the same target; fail clearly when no relative path exists (for
example different Windows drives), rather than silently returning absolute.

Implementation boundary: add a logical-target resolver and a small presentation
function taking `(target, format, base, caller_cwd, optional_render_context)`.
Do not change the existing configured-final get_path semantics globally. Library
callers pass context explicitly and never mutate process-global environment.

Known rendering context is supplied by the controlled renderer invocation, not
detected by path heuristics. Proposed transport: one versioned, private per-child
context snapshot file, referenced by per-command `PROJ_RUN_CONTEXT`. It contains
the shared captured root/profile/mapping/run-version/mode fields plus an optional
`render { engine, absolute_document_path }` descriptor for that renderer child.
Ordinary script/hook children receive no render descriptor. Reuse the existing
per-command environment mechanism; do not serialize all environment/config values
or credentials into the descriptor or machine stdout. Treat the snapshot as
immutable for that invocation; this is context transport, not a security boundary.
Nested path CLI calls consume it without re-reading changed YAML or VERSION;
their actual cwd is still obtained at lookup time. An explicitly selected
different project must reject the inherited run context instead of mixing roots.
Outside a run, resolve standalone context once at that query's entry.

Direct Quarto/Rmd/RStudio rendering does not gain this context automatically.
Without an explicitly supplied descriptor, auto remains ordinary-script mode;
the caller can request absolute representation. Future authorized frontends may
pass native render context explicitly. Existing projr helpers do not consume
PROJ_RUN_CONTEXT or PROJ_PROJECT_ROOT; no R wrapper/helper change is included.

For embedded HTML, feed the absolute auto result to normal include_graphics
handling, the smallest common successful candidate measured above. Do not force
rel_path=FALSE. Explicit relative output stays a filesystem path relative to its
chosen base; it is not promised to be a Markdown/resource URL. A separately
document-relative adapter remains a tested alternative, not an ordinary lookup
rule. Resolve effective embedding settings first: unspecified -> true, explicit
true/false unchanged, HTML only. This does not repair linked output or establish
PDF/GUI/direct-entry parity.

Acceptance before implementation is called complete: same target after cwd
changes into another project; profile/run version unchanged across nested child
queries and temporary VERSION transitions; auto differs only in representation
between script/render contexts; explicit bases/overrides round-trip correctly;
ordinary files named .qmd do not manufacture render context; actual embedded
sentinel survives relocation under both native and root execution. Existing
defensive guards/recovery remain until controlled cache-backed execution and
temporary VERSION/config/context restoration are separately verified.

Ponytail full design review: reuse per-command environment and native knitr
conversion; two pure concerns (target, presentation), one explicit context
descriptor. Deferred linked-bundle repair, new R bindings, renderer auto-detection
heuristics and additional diagnostics. Proposal requires design review before
production changes or a new binary.


## Literal Markdown and raw LaTeX — native semantics, no source rewrite

[Focused remote run39556599001](https://github.com/MiguelRodo/projcli/actions/runs/39556599001),
tested harness `ca76dba`, tool versions recorded in literal-summary.json.
This adds root documents, Quarto leading-slash comparisons and four actual
released-CLI builds; prior nested document-relative cases are reused from run
39554636201 rather than re-executed. All fixtures are disposable; no Git action
or storage publication was configured. A successful workflow means evidence is
complete, not that every author URL is valid or portable.

For this fixture layout, ordinary Markdown should be authored relative to the
document, independently of R/Python/Quarto code execution cwd:

| Document | Literal Markdown |
|---|---|
| root/report.qmd or report.Rmd | `![image](_raw/images/sentinel.png)` |
| root/analysis/report.qmd or report.Rmd | `![image](../_raw/images/sentinel.png)` |

The reused nested cases measured the literal Markdown sentinel separately from
code-generated figures: explicit embedded HTML retained exact image bytes under
both native and root execution in Rmd, standalone Quarto and Quarto project.
For linked HTML, copying only HTML+stem assets lost sibling raw-image references;
moving the complete Quarto project output tree retained that literal image under
both execution modes. These linked observations are not a new repair requirement.

### Additional executed cases

Counts: {"cases": 22, "render_process_failures": 0, "source_changed": [], "successful_render_missing_html": []}.

| Case | Render exit | Source unchanged | HTML+stem/captured published bundle: sentinel portable | Whole Quarto output: sentinel portable |
|---|---|---|---|---|
| literal-rmarkdown-root-relative-native-embedded | 0 | True | yes | not produced |
| literal-rmarkdown-root-relative-native-linked | 0 | True | no | not produced |
| literal-quarto-document-root-relative-native-embedded | 0 | True | yes | not produced |
| literal-quarto-document-root-relative-native-linked | 0 | True | no | not produced |
| literal-quarto-project-root-relative-native-embedded | 0 | True | yes | yes |
| literal-quarto-project-root-relative-native-linked | 0 | True | no | yes |
| literal-quarto-document-root-leading-slash-native-embedded | 0 | True | yes | not produced |
| literal-quarto-document-root-leading-slash-native-linked | 0 | True | no | not produced |
| literal-quarto-document-nested-leading-slash-native-embedded | 0 | True | no | not produced |
| literal-quarto-document-nested-leading-slash-native-linked | 0 | True | no | not produced |
| literal-quarto-document-nested-leading-slash-root-embedded | 0 | True | no | not produced |
| literal-quarto-document-nested-leading-slash-root-linked | 0 | True | no | not produced |
| literal-quarto-project-root-leading-slash-native-embedded | 0 | True | yes | yes |
| literal-quarto-project-root-leading-slash-native-linked | 0 | True | no | yes |
| literal-quarto-project-nested-leading-slash-native-embedded | 0 | True | yes | yes |
| literal-quarto-project-nested-leading-slash-native-linked | 0 | True | no | yes |
| literal-quarto-project-nested-leading-slash-root-embedded | 0 | True | yes | yes |
| literal-quarto-project-nested-leading-slash-root-linked | 0 | True | no | yes |
| literal-released-cli-Rmd-root-embedded | 0 | True | yes | not produced |
| literal-released-cli-Rmd-nested-embedded | 0 | True | yes | not produced |
| literal-released-cli-qmd-root-embedded | 0 | True | yes | not produced |
| literal-released-cli-qmd-nested-embedded | 0 | True | yes | not produced |

PDF availability/execution: {"engine": null, "reason": "No preinstalled TeX engine; no large installation attempted", "tested": false}.


Measured interpretation: all four released-CLI root/nested Rmd/QMD embedded
builds succeeded, and their published artifacts retained the exact image bytes
after source/cache became unavailable. All22 added renderer processes returned0
and all source bytes were unchanged; some leading-slash references were still
broken, so process success alone is insufficient.

In Quarto1.10.19, project `/_raw/...` worked for root and nested documents under
native/root execution. Standalone root `/_raw/...` also happened to work, but
standalone nested documents emitted `./_raw/...` and failed under both cwd modes.
It did not discover the surrounding proj root. Do not claim slash shorthand is
universally rejected outside a project or universally resolves proj root; use
the native project context explicitly and prefer document-relative markup for
standalone portability. Native linked Quarto whole-tree copies retained the
sentinel; minimal copies did not. Linked repair remains deferred.

### Intended projcli rule

Preserve native document-resource semantics for authored Markdown. Render at the
original source anchor or preserve a faithful source/resource layout while using
supported renderer controls to direct generated output into cache. Do not rebase
literal URLs to proj root, rewrite user markup, crawl and repair arbitrary links,
or silently retry a failed document-relative reference against root. A code
execution-directory toggle changes code cwd, not the author's Markdown base.

Quarto project `/...` shorthand is native project syntax only where the actual
renderer supports it; the active Quarto project root must equal proj root. Do not
invent that interpretation for standalone documents, Rmd, generic filesystem
paths, HTML server-root URLs or raw TeX. The version-specific measurements above
show actual behavior and must not be promoted into a cross-renderer guarantee.

Generated proj paths are different: resolve the logical target from captured
config/root/profile/run version, then choose filesystem representation as proposed
above. A known rendering adapter can pass an absolute image to normal
include_graphics. That does not modify literal source URLs elsewhere in the file.

Embedded output consumes the image during the build, so validate actual payload
after controlled promotion with source/cache unavailable. Explicit false remains
respected; linked output can retain dependencies and may break after relocation
in this early scope. No new linked-bundle repair or URL rewriting is authorized.
The literal tests use explicit true/false; they do not implement the proposed
unspecified->true fallback in production.

### Raw LaTeX boundaries

Raw `\includegraphics{...}` is a TeX/compiler instruction, with native compiler
cwd, generated .tex location and graphics/search-path configuration. It is not
an ordinary Pandoc Image node to which projcli should apply project-root lookup
or Markdown URL rewriting. A leading slash is not automatically Quarto's Markdown
project shorthand inside raw TeX. Preserve native behavior; verify the actual PDF
toolchain before promising a relative-base rule for a particular entrypoint.

The [Pandoc raw-TeX documentation](https://pandoc.org/MANUAL.html#extension-raw_tex)
says TeX is passed through to applicable writers and inline LaTeX is ignored by
most other output formats. Raw LaTeX graphics therefore are not a universal HTML
image mechanism; use ordinary Markdown images for multi-format documents. A
successful HTML render containing raw TeX alone would not establish image inclusion.
No large TeX installation was attempted. PDF image payload verification, GUI,
Windows/macOS real rendering and direct quarto R API parity remain untested unless
the explicit execution record above states otherwise (a PDF header alone is not
image-payload proof).

Ponytail full review: reused completed nested evidence and the existing fixture,
HTTP/hash inspector, native tools and released checksum-pinned binary. Added no
production parser, crawler, renderer override, R wrapper or bundle-repair feature.
Original source bytes are checked in every added case; prod Rust/R/thesis remain
unchanged. The diagnostic workflow/branch is not intended to merge as a replacement
for the production test suite.

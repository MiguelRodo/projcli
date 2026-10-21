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

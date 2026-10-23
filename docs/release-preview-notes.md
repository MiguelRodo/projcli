# projcli 0.4.0-preview.3

This development preview adds ordinary local-folder and GitHub Release storage
to the existing local analysis workflow. The exact tagged source is recorded in
source-commit.txt; checksums.txt identifies both platform downloads. These notes
are prepared before publication and do not claim that installation has happened.

## What changed since preview.2

- Save selected declared outputs to a current destination (latest) or keep
  versioned snapshots (archive), then restore one declared label.
- Choose when to send files (if-change or always), compare recorded history or
  actual files, or explicitly skip comparison. The four supported transfer
  strategies control adding, replacing and removing remote files.
- Use GitHub Releases through native HTTP without requiring GitHub CLI. Data
  and shared CSV/VERSION/changelog metadata use projr-compatible ZIP names.
  Ordinary manifest.csv records are independent of the existing CAS groundwork.
- Stage and validate downloads before replacing local files. Ambiguous names,
  unsafe ZIP entries and overlapping publication destinations fail closed.
  Publication errors report partial progress and preserve the local release.
- Retain explicit-empty-script upload-only builds and development previews
  that do not publish. Invalid script exclusions and root-enumeration errors
  fail clearly; real-Git and failed-render/retry regressions are covered.

The complete GitHub layout/inspection/strategy/cue matrix is exercised with real
CLI commands against mocked REST. A separate synthetic real GitHub round-trip
passed latest replacement and restore, archive history reuse, changed snapshots,
exact data/metadata checks and preservation of older assets. It does not prove
all combinations against live GitHub. Evidence:
https://miguelrodo.github.io/projcli/remote-compatibility.html#live-github-fixture-verification

## Existing workflow and limits

The configured local R/Python/report workflow, safe versioned cache defaults,
explicit output management and native renderer behavior remain supported.
Existing R helpers/packages and thesis files are unchanged. Direct R/IDE parity,
Bookdown site embedding changes and linked-output portability are not claimed.

Restore currently requires an explicit single label. Default multi-label raw
restore, separate individual-file/list-files/trusted-version conveniences,
additional credential-helper/repository inference and local configured
path-append-label:false remain follow-up work. Existing token mechanisms are
supported; no credentials are installed or permissions granted by proj.

Remote replacement is not atomic. Errors can follow a successful removal or
some uploads; do not infer full rollback or automatically retry a failed
publication. Windows read-only backup cleanup may fail after installation;
diagnostics distinguish that from a retained original after rollback failure.
Recovery does not isolate arbitrary scripts, concurrent writers or power loss.

Complete provenance, durable attempt/release records, analysis-free publication
retry and M2 remain unfinished. The proposed remote registry and CAS-specific
snapshot discovery/retention remain separate design work. No new defaults,
manifest identity policy or mandatory DVC dependency are introduced.

Linux/WSL x86-64 and Windows x86-64 binaries are supplied. Linux requires glibc
2.35 or newer; macOS/ARM binaries are not supplied. Installation needs no Rust
compiler or administrator access. Use sample data or a backed-up disposable copy.

Release checks:
https://miguelrodo.github.io/projcli/release-candidate.html

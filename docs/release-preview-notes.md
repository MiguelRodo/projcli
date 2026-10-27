# projcli 0.4.0-preview.5

This preview adds experimental Xet storage and complete CAS/Xet store transfer
through GitHub Releases, plus the R interface and optional R build tasks introduced
since preview.4. Release assets include source-commit.txt and checksums.txt; use
those files to identify the exact reviewed source and platform download.

## What changed

- From R, preview/build projects, query configuration and resolve file paths using
  the separate thin projcli package. See the website's R package guide and function
  reference. It is available from source, not CRAN; the standalone command is
  installed separately. The original SATVILab/projr package is unchanged.
- Optional build.r actions generate documentation, render README.Rmd and snapshot
  renv dependencies. Each is off by default. Snapshot ordering follows projr.
  Explicit init readme --rmd and init renv commands do not replace existing state.
- Optional local structure: xet shares chunks within files using a separately built
  exact-pinned helper. Upstream's local backend is experimental. The core verifies
  independent whole-file BLAKE3 and length in private staging before installing
  restored files; successful upstream return values alone are insufficient.
- GitHub structure: xet uses the same local engine through complete-store ZIPs.
  GitHub structure: cas transports the existing whole-file store and preserves
  tree, flat CSV/JSON/dictionary and legacy snapshots. Ordinary latest/archive
  uploads and local file CAS remain compatible.
- Each configured store uses one long-running storage release. A new immutable,
  checksum-named ZIP is uploaded, downloaded and validated before its trusted
  project reference can change. Prior good assets are retained. Failed or uncertain
  operations can leave an unreferenced ZIP; they do not delete the old store.
- The website introduces the R package prominently and provides storage guides.
  Changed-source CI checks Rustfmt/Clippy and Air/lintr without contributor writes.

## Requirements and limits

Xet runtime is currently Linux/Unix only, needs the separately pinned optional
helper, and is not a production archival guarantee. No Hugging Face account or
service is required for this local engine. File/chunk sharing depends on actual
bytes; already-compressed files may share little. Xet snapshot inventories remain
complete per-version documents; chunk sharing is not metadata delta encoding.

GitHub transfers complete ZIPs, not incremental chunks. Initial configured
support requires full metadata in the remote store (manifest: auto or remote);
unsupported placement/style options fail before hooks. GitHub CAS requires an
explicit manifest-style: tree or flat. Both use project Git metadata to identify
the trusted ZIP; missing project authority is an error, not permission to guess
a latest export. Back up that metadata. Each store belongs to one project.

Exports require stopped/cooperative writers; no hot-copy, power-loss, distributed
locking, automatic migration, retention cleanup or garbage collection guarantee.
Outer hashes, strict ZIP membership/path/size checks and deep reconstruction
verification precede guarded installation. Temporary disk requirements can exceed
the ZIP size, and sparse databases can expand. Arbitrary scripts remain able to
modify files; this is not a sandbox.

The unrelated pending initialization/scripts PRs are not part of this release.
No new general citation helpers, DESCRIPTION synchronization or glob/routing
features are claimed here. End-hook mutation enforcement remains deferred.

## Verification

Final exact-head/main, platform, real synthetic GitHub round-trip, independently
verified artifacts, release checksum and installed-path results must be attached
to the delivery checkpoint before this candidate is described as published.
Synthetic tests do not establish all real workload savings or whole projr parity.

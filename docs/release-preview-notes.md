# projcli 0.4.0-preview.4

This development preview adds inspectable build records and verified local
recovery, including the approved remote configuration and file-list choices.
Exact source: f6f4510a806ea302a2ceee5177707fbde9bed5bf. Both platform downloads
are identified by checksums.txt; source-commit.txt binds them to this source.

## What changed since preview.3

- Define named backup locations once, then choose the locations and declared
  folders each build sends. Removing an upload destination does not remove
  that location as a restore source.
- Save readable CSV or JSON file histories. An unchanged folder version points
  directly to an earlier complete listing. Optional file dictionaries shorten
  repeated content hashes while retaining every current file path. CSV stays
  rectangular for spreadsheet filtering; a reference row identifies the earlier
  version to filter.
- Choose where full listings live: project, remote, both, or automatic. Even
  remote-only metadata keeps a small project reference. Existing ordinary root
  manifest.csv and projr-compatible transport metadata retain their separate
  meanings and are not removed by this setting.
- Save content-addressed local archives: identical file bytes share storage and
  directory fingerprints describe their complete membership. Tree metadata is
  the modern local CAS default; ordinary local and GitHub backups default to
  flat listings. Explicit style changes preserve old snapshots/readers.
- Verify and restore an explicitly selected local snapshot after losing its
  working project. Required metadata and bytes are checked before replacing
  a good destination. Earlier flat CAS IDs remain readable; no destructive
  conversion occurs. Older lookup records keep their original bytes on exact
  retries, and shared lookup updates are coordinated across stores.
- Keep useful per-attempt records, source/configuration/dependency fingerprints,
  inspectable final file rows and human-readable history. Failed builds restore
  the exact prior development version where rollback applies. Failures with
  possible distribution keep evidence; earlier failures retain local diagnostics
  excluded from Git. Optional Git records a pre-commit snapshot and saves the
  final Git outcome locally for the next normal commit, without recursive commits.

## Verification and boundaries

The exact release source was checked remotely with all-platform Rust suites, API examples,
installers, native browser documentation and actual R/Python/HTML analysis.
The bounded M3 gate combines an actual analysis, ordinary backup and native local
archive, removes only a disposable working project, then restores both selected
labels by explicit snapshot ID. Retained bytes and corruption failures have a
separate Rust verification step. The integrated gate and its independent byte checks passed.

New GitHub listing/tree formats use actual CLI commands against mocked REST.
The earlier synthetic real GitHub transport round-trip remains evidence for its
tested latest/archive operations, not a new live test of every metadata format.
Ordinary data ZIP names, CSV/VERSION metadata, transfer decisions and guarded
single-label restore retain their documented compatibility behavior.

A remote store currently belongs to one project. Automatic snapshot discovery,
garbage collection, retention redesign, chunking, network packing and performance
optimisation are deferred. Hashing guarantees observed bytes/membership, not a
live consistent dataset or ownership/permission restoration. No size or speed
saving is promised for JSON or tree metadata. Full metadata is still needed for
recovery; a small root reference alone is insufficient.

Publication is not an atomic transaction across locations or GitHub assets.
A failure can leave earlier uploads or complete metadata copies; diagnostics
preserve the local release and possible distribution evidence. Do not infer
full rollback or automatically retry an ambiguous failed publication. Existing
Windows backup-cleanup and arbitrary-script/power-loss limitations remain.
Project builds and mutable CAS history/lookup updates use cooperative writer claims;
standalone ordinary file-copy operations keep their documented concurrency limits.
This is not a distributed lock.

The existing R package, helpers and thesis are unchanged. Language wrappers,
R/IDE parity, richer initialisation, Bookdown site embedding and linked-resource
portability remain separate work. Ordinary restore is still single-label.
Individual-file/listing/trusted-version conveniences and additional credential
inference remain follow-ups. Git is optional for ordinary local work and is not
bundled; no credentials or permissions are installed. DVC is not required.

Linux/WSL x86-64 and Windows x86-64 binaries are supplied. Linux requires glibc
2.35 or newer; macOS/ARM binaries are not supplied. Installation needs no Rust
compiler or administrator access. Evaluate with sample data or a backed-up copy.

Reference and delivery checks:
https://miguelrodo.github.io/projcli/release-candidate.html

Exact-tag Linux/Windows tests, binary version/protocol and installer checks:
https://github.com/MiguelRodo/projcli/actions/runs/40124525153

Full pinned acceptance on the identical reviewed release tree:
https://github.com/MiguelRodo/projcli/actions/runs/40122746225

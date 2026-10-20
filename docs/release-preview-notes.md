# projcli 0.4.0-preview.1 — development preview

Preview/development build for disposable projects and sample data.

This release is intended for testing the supported Rust CLI preview workflow.
It is not a declaration that M1/M2 or the original roadmap is complete.

Supported delivery: Linux x86-64 (built on Ubuntu 22.04; glibc 2.35 or newer) and
Windows x86-64. Other platform binaries are not supplied by this preview pipeline.
Each versioned binary has an exact SHA256 entry in checksums.txt; version.txt binds
installer asset selection to this release. A checksum detects corrupted/mismatched
downloads; it is not an independent signature or reproducible-build attestation.

Included Rust pieces: explicit/nearest-root project selection, active YAML/profile
validation, explicitly declared resource paths, local-only initialization, ordered
scripts/hooks with a captured execution root, and protocol-v1 machine results.
Real Python and mocked renderer tests cover the supported CLI boundaries; complete
real R/renderer/IDE end-to-end parity is not established.

Use explicit active configuration and declare every resource directory used.
Machine capabilities report implemented protocol features. Preview recovery protects
captured managed paths, not arbitrary external script side effects or concurrent
writers. Keep independent backups and test on disposable copies. Direct R/IDE
parity, automatic output redirection, durable attempt/release records and analysis-free
publication retry remain unfinished. Existing R package is unchanged.

The pipeline creates a draft prerelease only after exact-tag tests, binary smokes,
installer verification and checksum collection pass. Publication and user-local WSL
installation follow separate verification of the actual uploaded assets.

This candidate includes captured managed-state recovery after successful and failed
previews, retaining cache artifacts. Capture/recovery has no concurrent-writer or
power-loss atomicity guarantee and does not undo arbitrary external script effects.
M1/M2 remain incomplete; the package version is not a scientific release identity.
Publication remains gated on the final reviewed main checks and verified draft assets.
Current status: https://miguelrodo.github.io/projcli/preview-status.html


Delivery verification completed: [exact-tag workflow](https://github.com/MiguelRodo/projcli/actions/runs/39515169842), complete uploaded
and public asset checks, both native version/protocol smokes, and a user-local WSL
installation with disposable successful/failing Python previews. Source commit:
`bbd12edc8de9865a96b6145d0c7702f1300cb1a8`. Stable Latest v0.3.0 is unchanged.

# Experimental optional Xet helper

Use only with independent backups. Upstream describes LocalClient as a disk-backed
testing/development backend; no supported archival/format-compatibility guarantee
has been established. The candidate is not production archival-ready.

The helper uses unmodified upstream modules at commit
629562cd9fc37432317eb57811faf13a6935c601, original Cargo.lock SHA-256
ecb897ce0eaa6e3d2ec4f55ab903104d3ebe4345b7a0c8752d2b16d381c7fd7c,
and Rust 1.95.0. Root proj's Cargo dependencies remain unchanged.
The workspace lock has 581 entries, not 581 necessarily linked dependencies.
Only explicit building fetches anything; ordinary proj never auto-installs it.

Helper protocol1 has info and private upload/download operations. Finalize must
return before an upload pointer is written. A download's success is not evidence
of integrity: upstream qualification accepted corrupted bytes. Core reconstruction
checks trusted whole-file BLAKE3 and length independently before guarded install.
The upstream Merkle identifier is an address, not that full-file digest.
Child failures/panics are errors; a ten-minute per-operation timeout owns/reaps
only the directly spawned helper. Abrupt parent/system termination is not covered.

Mutable store access rejects symlinks/special files and multiply-linked files.
Unix is the current experimental runtime; equivalent Windows hard-link validation
is pending. No default supported backend or existing store is changed.

Upstream is Apache-2.0. Retain its licences/attributions and modification notices.
Pinned tree has LICENSE and hf_xet/LICENSE, no NOTICE. A transitive dependency
license inventory is required before distributing a built helper; this has not
been completed. Upstream source preservation is separate from integration code.
Full local pipeline reuse is a shorter experiment; using lower-level chunk/hash/
codec primitives instead requires projcli-owned persistence/index/recovery.

Build script currently requires an isolated source directory and output path.
It is not an installation command. Registry dependencies/toolchains can still
need network; source preservation alone does not preserve registry tarballs.

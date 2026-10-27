# Controlled interruption and closed-store export qualification

Official upstream and toolchain remain629562cd9fc37432317eb57811faf13a6935c601 /
Rust1.95.0. Baseline run40459341098 and its source6d931083 remain unchanged.

This branch is qualification only, not a production backend, product format,
migration, release or installation. Remote CI uses the same2CPU/6GiB/40min cap.

## Controlled process failures

Each disposable store starts with committed synthetic version A. An independent
temporary oracle derives B's actual Xet pointer, then its store is deleted.
Four writer processes stop at explicit public-API barriers:

1. Half the input consumed, cleaner checkpoint and session checkpoint returned.
2. Complete file cleaner finish returned, session finalize not called.
3. Session finalize returned; client deliberately kept open; no external reference.
4. Finalize returned and a harness reference was fsynced, renamed and its directory
   fsynced; client still held open.

Only the controller's own remote fixture process is terminated with SIGKILL.
No process search, user process, service or host shutdown is involved. We wait
for termination; restart/download operations have finite timeouts. No manual
lock deletion, index repair or alternate backend is used.

A must remain exact at every stage. Partial B must fail clearly. Before finalize,
a complete orphan B may exist or lookup may fail, but an apparent successful read
must have exact expected bytes. After finalize B must be exact. External reference
presence must match the stage. Retry into the same crashed store must preserve A
and produce exact B. This tests these precise API boundaries, not arbitrary
instructions inside a redb transaction, machine power loss, filesystem failure
or every possible background race.

External reference fsync/rename and commit ordering are supplied by the harness,
not an upstream project-version/snapshot transaction guarantee.

## Missing/corrupt content

Separate seed copies lose A's actual payload pack or receive a flipped byte in
that pack. Reconstruction must fail nonzero; success with wrong bytes is a hard
failure. Output is private disposable staging. Partial staging may remain on
upstream failure; a production adapter still needs verify-before-promotion.

## Portable closed-store ZIP

High-entropy and gzip-compressed pairs are uploaded with all writer processes
exited. Client shard-cache/session directories are removed while durable
server-side redb/shards/packs remain. ZIP contains the durable store, two pointers,
and a bounded SHA256 membership manifest (an experimental transport envelope only).

Original working files/store/references are then deleted. A fresh import rejects
unsafe/duplicate ZIP members, verifies full expanded membership, sizes and hashes,
and reconstructs both versions byte-exact in new processes without client caches.
No merge into an existing store or concurrent hot-copy is tested. Physical/apparent
store size, ZIP bytes, expanded bytes and imported allocation are reported:
ZIP preserves bytes, not sparse allocation. Permissions/ACL fidelity and
crash-atomic GitHub publication are outside this experiment.

Evidence is JSON/log/provenance only; synthetic stores and ZIP payloads stay in the
disposable runner. No HF account, remote storage service or live asset is used.

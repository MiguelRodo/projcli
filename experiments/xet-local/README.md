# Isolated actual-Xet local experiment

Experiment only: no projcli backend, archive migration, merge or user installation.
Official huggingface/xet-core commit629562cd9fc37432317eb57811faf13a6935c601,
Rust1.95.0 (upstream CI pin), Cargo.lock SHA256
ecb897ce0eaa6e3d2ec4f55ab903104d3ebe4345b7a0c8752d2b16d381c7fd7c.

The workflow adds probe.rs as a normal upstream binary, calling the actual
FileUploadSession/FileDownloadSession with TranslatorConfig::local_config,
auth=None and no download chunk cache. Xet's own chunker, shards, compression and
disk-backed LocalClient remain unchanged. This upstream local backend is intended
for development/testing; normal restart success is not crash-durability proof.

Independent stores use deterministic high-entropy, compressible and gzip inputs:
v1, unchanged v1, changed v2, unchanged v2, each in a separate OS process.
Measure all files including redb/shards/client metadata and report apparent bytes,
allocated blocks and pointer metadata separately. Compare whole-file-deduplicated
raw and gzip-level6 baselines including matching four version-pointer records.
Metadata-inclusive Xet ratios include its external pointer files too. Default Xet compression is enabled: physical savings
are not all deduplication; gzip is a control, not the same compressor. Filesystem
inode/superblock overhead is not attributable and copying can alter sparse allocation.

After process exit, copy the quiescent store, remove declared client cache/session
directories, delete original store/working files/baselines, and reconstruct both
versions from relocated durable storage plus pointers in fresh processes. Verify
full SHA256 and length. Fresh HOME/TMP and minimal child environment per process;
no HF login, auth token, network service or download chunk cache.

Remote CI only: existing pinned image,2CPU/6GiB, Cargo jobs2,40min job/25min build/
10min workload.16MiB originals,64KiB insert+32KiB replacement. Artifacts retain only
metadata, harness, lock/provenance and resource measurements, never payload/store.
Pending PR232/233/234 remain untouched.

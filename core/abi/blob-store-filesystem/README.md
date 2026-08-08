# Hoplite trusted-root filesystem blob store

`hoplite-blob-store-filesystem` is a restart-safe implementation of the generic
`hoplite_blob_store::BlobStore` contract beneath the installed `hara.blob`
service.

It contains no Tahto application, namespace, quota, graph, manifest,
authorization, receipt, replication or merge semantics.

## Trusted installation boundary

The storage root and all limits are selected by trusted Hoplite configuration:

```rust
let store = FilesystemBlobStore::open(configured_root, configured_limits)?;
```

A HAL call can supply only the closed generic blob request fields. It cannot
select a path, bucket, provider, driver, native library, credential or command.

Every physical path below the root is provider-derived:

```text
<root>/
├── store.lock
├── staging/
│   ├── <sha256(logical-staging-key)>.meta
│   └── <sha256(logical-staging-key)>.data
└── objects/sha256/
    └── <first-two-hex>/
        ├── <remaining-hex>.meta
        └── <remaining-hex>.blob
```

Logical staging keys are recorded in bounded metadata and are never interpreted
as paths.

## Restart and transaction model

Staging metadata records an independently committed offset. An append proceeds
in this order:

```text
validate exact offset
consume and finish the bounded source
append bytes
fsync staging bytes
atomically replace metadata with the new offset
fsync the staging directory
```

If a process stops after bytes are written but before the metadata offset is
installed, the next open truncates the uncommitted tail to the last durable
offset. If completion delivery is lost after metadata replacement, reopening
returns the new offset.

A commit:

```text
checks complete size
streams SHA-256 over actual staged bytes
fsyncs staged bytes
hard-links bytes to the digest-derived final identity without replacement
writes and fsyncs bounded object metadata
fsyncs the object directory
removes staging metadata and bytes
```

The hard link keeps installation on one trusted filesystem and makes the object
visible without copying or exposing a partially written final object. A crash
after the object link but before object metadata is recoverable by repeating the
same verified commit.

## Concurrency

Every operation is serialized through both a process mutex and an advisory lock
on `<root>/store.lock`. Independent worker processes therefore re-read durable
metadata before applying an offset or capacity decision. A stale writer fails
without consuming its source.

This first profile deliberately uses one root-wide lock. Finer-grained staging
and object locks may replace it later without changing the public `BlobStore`
contract.

## Integrity

The driver:

- recomputes SHA-256 over staged bytes before installation;
- recomputes SHA-256 before returning an immutable source;
- verifies stored metadata, object size and digest-derived physical identity;
- rejects symlink roots and provider-owned symlink entries;
- rejects unsupported, truncated, oversized and trailing metadata;
- detects object-byte tampering before a source handle is registered;
- preserves the first committed media type for a globally deduplicated digest;
- treats an exact already-installed object as idempotent success; and
- never exposes a physical delete operation through the generic capability.

## Response sources

`object_open_source` returns a seeked, bounded `FilesystemResponseSource`. It
will return at most the authorized range length, reports early EOF as an
integrity failure and closes exactly once through the generic response-source
lifecycle.

Nginx backpressure and output-filter resumption remain the separate transport
slice tracked by Hoplite #42.

## Conformance

The standalone Rust 1.78 suite covers:

- layout creation, reopen and explicit verification;
- root and child symlink rejection;
- staged append and resume across process restart;
- exact source finish, short source and long source handling;
- two-instance stale-offset rejection;
- recovery of an uncommitted append tail;
- recovery after object bytes are linked but metadata is absent;
- atomic commit, exact replay and bounded range reads;
- object tamper detection;
- idempotent abort; and
- cleanup of orphan staging bytes.

The next PR binds this trusted-root driver to the already-registered
`hara.blob` provider through installation configuration while retaining the
in-memory driver for deterministic provider tests.

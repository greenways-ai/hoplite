---
title: Provider distributions
description: Compose native Hoplite providers from verified artifacts and trusted worker configuration.
---

Hoplite's default production server contains the provider registry and portable
transport, but not a storage, blob, canonical-value, secret, or authorization
implementation. A provider-enabled distribution chooses which implementations
to compile, links their Nginx lifecycle modules, and supplies trusted startup
configuration.

## Composition boundary

A native provider descriptor declares an ABI version, exact service name,
invoke callback, optional cancellation callback, and request- or response-body
capabilities. Registration happens during worker initialization and fails on
ABI mismatch, duplicate service names, invalid callbacks, or registry overflow.

Provider paths, limits, credentials, and driver identity belong to the process.
They must not appear in portable HAL requests. A missing optional provider
leaves its service unregistered; a partially configured or invalid selected
provider fails worker startup.

## Blob provider artifact 0.1.1

The filesystem `hoplite.blob` implementation is published independently as a
target-neutral deterministic source artifact. Version `0.1.1` is the first
pinned artifact containing the shared verified filesystem reader used by both
blob range egress and canonical-value verification.

```text
tag       hoplite-blob-provider-v0.1.1
asset     hoplite-blob-provider-0.1.1.tar.gz
source    8d97a452032d47740314899bb175096d8fc83f8e
SHA-256   03c5dea9854cf23b60c7d2638c17712accc7e77eb53db4d15ed0b45327ee8210
```

The source-tree manifest describes the closed provider contract but keeps its
artifact digest `null`: an archive cannot contain its own final digest. The
published manifest binds the archive digest. `provider-lock.json` additionally
binds the provider version, source revision, repository, tag, asset name, and
media type.

A distribution must:

1. Validate the published manifest and provider lock as closed JSON contracts.
2. Require the two documents to name the same artifact digest.
3. Download the exact versioned release asset and verify its SHA-256 before extraction.
4. Reject unsafe archive paths, links, duplicate entries, and files outside the closed inventory.
5. Verify the extracted source revision and package version.
6. Build the provider independently and link the distribution-owned Nginx lifecycle module.

The reference composition is
`packaging/docker/Dockerfile.blob-provider`. Release-host mutability is not a
correctness dependency because the reviewed lock, digest, inventory, source
revision, and compatibility manifest all fail closed.

## Trusted worker configuration

### Filesystem blob provider

| Variable | Purpose |
| --- | --- |
| `HOPLITE_BLOB_ROOT` | Trusted filesystem store root; required by the blob-provider distribution |
| `HOPLITE_BLOB_MAX_OBJECT_BYTES` | Maximum committed object size |
| `HOPLITE_BLOB_MAX_APPEND_BYTES` | Maximum bytes accepted by one append |
| `HOPLITE_BLOB_MAX_SOURCE_CHUNK_BYTES` | Maximum request-source read chunk |
| `HOPLITE_BLOB_MAX_STAGING_KEY_BYTES` | Maximum staging-key length |
| `HOPLITE_BLOB_MAX_MEDIA_TYPE_BYTES` | Maximum media-type length |
| `HOPLITE_BLOB_MAX_STAGING_ENTRIES` | Maximum staged entries |
| `HOPLITE_BLOB_MAX_OBJECTS` | Maximum installed objects |

### Canonical-value provider

| Variable | Purpose |
| --- | --- |
| `HOPLITE_VALUE_PROVIDER` | Must be `filesystem` when the provider is enabled |
| `HOPLITE_VALUE_ROOT` | Trusted blob root shared with immutable object custody |
| `HOPLITE_VALUE_MAX_BYTES` | Maximum canonical value frame accepted from storage |

All three value-provider variables must be absent or valid together. Partial
configuration fails startup.

### SQLite store provider

| Variable | Purpose |
| --- | --- |
| `HOPLITE_STORE_PATH` | SQLite database path; required by a store-only distribution |
| `HOPLITE_STORE_MAX_VALUE_BYTES` | Maximum opaque canonical value frame; defaults to 8 MiB |
| `HOPLITE_STORE_MAX_RECEIPT_BYTES` | Maximum opaque receipt frame; defaults to 1 MiB |

The store-only lifecycle registers exactly `hoplite.store` and does not link or
initialize `hoplite.value`. The older combined value/store lifecycle remains a
compatibility wrapper for distributions that deliberately install both
services.

## Operational checks

Persist provider data separately from the embedded server cache. Validate
provider configuration before replacing the process with Nginx, reject
malformed positive decimal limits, and treat duplicate registration or an ABI
mismatch as startup failures. Provider shutdown must close worker-owned contexts
and any remaining response sources.

See [Data-plane providers](/concepts/data-plane-providers/) and
[Native provider protocols](/reference/data-plane-protocols/).

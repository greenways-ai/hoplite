---
title: Native provider protocols
description: Versioned portable request and result boundaries for Hoplite storage providers.
---

Native providers receive an operation name plus one canonical HTA1 argument
vector containing an exact closed request map. The host operation and request
operation must agree. Unknown and extra fields, malformed identifiers,
non-canonical digests, invalid ranges, and zero handles fail before driver
execution.

## `hoplite.blob`

Contract versions are `hoplite.blob-request/1` and
`hoplite.blob-result/1`. Supported operations are:

| Operation | Mechanical purpose |
| --- | --- |
| `staging/open` | Open or resume a bounded logical staging identity for an expected digest, size, and media type |
| `staging/append-from-source` | Append the exact declared range from a request/work-scoped source handle |
| `staging/abort` | Abandon a staging identity and its partial bytes |
| `staging/verify-commit` | Recompute size and SHA-256, then install the immutable object |
| `object/open-source` | Open one validated non-empty byte range as a response-source handle |

Staging keys are bounded logical identifiers, never filesystem paths. Object
digests use canonical lowercase `sha256:<64 hex digits>`. The protocol contains
no namespace, quota, graph, manifest, authorization, or recovery policy.

`staging/append-from-source` can resolve its handle only under the owning
request and work. `object/open-source` returns only the validated digest,
offset, length, and positive provider-owned source handle.

## `hoplite.store`

Contract versions are `hoplite.store-request/1` and
`hoplite.store-result/1`. Supported operations are:

| Operation | Mechanical purpose |
| --- | --- |
| `load` | Return the current revision and opaque canonical value, or nil when absent |
| `initialize` | Publish an initial exact value idempotently |
| `compare-and-swap` | Atomically replace the expected revision and publish an opaque receipt |
| `receipt` | Return prior receipt evidence, or nil when absent |

Nested values and receipts remain exact canonical HTA spans. The provider
verifies claimed value digests, revision progression, stale writers, receipt
replay, and collision rules without decoding an application state model.
Application HAL decides what a receipt means and whether an applied or replayed
result is a semantic success.

## `hoplite.value`

`object/verify-hta` accepts `hoplite.value-request/1` and returns
`hoplite.value-result/1`. It reads one immutable object selected by digest,
enforces the request maximum, verifies actual bytes and SHA-256, and decodes one
canonical portable `hara.hta/1` value. See
[`hoplite.value`](/reference/hoplite-value/) for the HAL validators.

## Ownership and failures

Provider paths, SQL, credentials, driver identity, and native error details do
not enter these contracts. Normal provider failures return stable closed codes;
ABI pointer, frame, ownership, or panic failures terminate at the native
boundary without constructing an application result.

See [Data-plane providers](/concepts/data-plane-providers/) and
[Provider distributions](/guides/provider-distributions/).

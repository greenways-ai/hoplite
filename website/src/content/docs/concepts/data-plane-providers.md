---
title: Data-plane providers
description: How portable HAL policy reaches distribution-owned storage and streaming services.
---

Hoplite core owns a small provider-neutral transport. A distribution may
install implementations of four related boundaries:

| Boundary | Purpose | Authority it does not grant |
| --- | --- | --- |
| `hoplite.store` | Load and atomically replace opaque canonical values | Application transitions, authorization, or database selection |
| `hoplite.blob` | Stage, verify, commit, and open immutable byte ranges | Namespace, quota, graph, or upload policy |
| `hoplite.value` | Verify one immutable object as a bounded canonical Hara value | Schema selection, semantic validation, or object authorization |
| `hoplite.response-source` | Describe an already-open bounded response body | A path, provider choice, credential, or portable handle |

Applications communicate with provider services through exact operation names
and standalone canonical HTA0 argument frames. Provider-specific results are
returned as closed portable contracts or stable failure codes; filesystem
paths, SQL, credentials, native pointers, and operating-system errors do not
cross into HAL.

## Worker-local registration

Each Nginx worker owns one fixed-capacity provider registry. Trusted
distribution code registers immutable service descriptors during worker
startup. Lookup is an exact, case-sensitive service-name match.

Application code can call a registered service, but it cannot register or
replace one. A portable request cannot select a dynamic library, path, driver,
bucket, credential, maximum, or provider identity. Those remain trusted
startup inputs.

## Resource authority

Native request bodies and response sources are scoped by all of:

```text
opaque request context + owning work ID + live handle + provider capability
```

The numeric handle alone is not authority. A provider cannot resolve a handle
from another request or work, and it loses access after synchronous completion,
asynchronous completion, cancellation, or cleanup.

`hoplite.blob` may consume a request-body handle while staging bytes and return
a response-source handle when opening an immutable range. Nginx retains
ownership of HTTP flow control; the provider retains ownership of the source
until it is closed.

## Shared verified reads

The filesystem blob driver owns staging, fsync, restart recovery, and immutable
installation. Its read path and the filesystem `hoplite.value` provider share
one read-only implementation that:

```text
canonical SHA-256 digest
  -> trusted objects/sha256 path
  -> shared store.lock
  -> exact HBO0 metadata
  -> bounded actual read
  -> byte-length and SHA-256 verification
```

The shared reader proves byte identity. `hoplite.value` additionally proves
that the bytes contain one bounded canonical portable Hara value. Application
policy still decides whether that object and decoded value are authorized and
semantically valid.

See [Requests and responses](/concepts/requests-responses/),
[Provider distributions](/guides/provider-distributions/), and
[Native provider protocols](/reference/data-plane-protocols/).

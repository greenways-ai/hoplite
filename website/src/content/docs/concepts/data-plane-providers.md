---
title: Data-plane boundaries
description: Generic request and response ownership for trusted Hoplite embeddings.
---

Hoplite owns provider-neutral transport: a fixed worker-local host registry,
bounded canonical argument frames, explicit completion/cancellation, and
request/response ownership. It does not ship a concrete storage, credential,
identity, or provider product.

## Worker-local registration

Trusted embedding code may register immutable application-neutral service
descriptors during worker startup. Lookup is exact and case-sensitive.
Application code can call a registered service, but cannot register or replace
one or select a dynamic library, path, driver, bucket, credential, maximum, or
provider identity from request data.

## Resource authority

Native request bodies and response sources are scoped by:

```text
opaque request context + owning work ID + live handle + declared capability
```

The numeric handle alone is not authority. It cannot be resolved from another
request or work and expires after completion, timeout, cancellation, disconnect,
or cleanup. Nginx retains HTTP flow control; a source owner retains its source
until exactly-once close.

The public generic profiles are `hoplite.request-body/3` and
`hoplite.response-source/0-alpha`. Historical `hoplite.blob`, `hoplite.store`,
and `hoplite.value` products were retired in 0.2.0 and are not public contracts.

See [Requests and responses](/concepts/requests-responses/) and [Host
capabilities](/concepts/host-capabilities/) for the supported boundary.

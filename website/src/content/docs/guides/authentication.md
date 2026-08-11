---
title: Application authentication
description: Own authentication and authorization in HAL while using narrow native cryptographic capabilities.
---

Hoplite's default release does not create accounts, sessions, management users,
or application principals. It also does not accept `:hoplite/authentication` or
`:route/auth` as transport configuration. Authentication and authorization are
application policy and run explicitly in the HAL handler or a HAL package that
the handler calls.

This keeps the transport boundary honest: selecting a route does not silently
grant an identity, and installing a storage provider does not authorize an
application operation.

## Native mechanics available to policy

The `hoplite.host` namespace exposes bounded mechanics that are difficult or
unsafe to reproduce in application code:

```clojure
(host/random-bytes size)
(host/hash "sha256" value)
(host/canonical-value-digest value)
(host/base64url-decode value)
(host/hex-decode value)
(host/hex-encode value)
(host/p256-jwk-sec1 jwk-json)
(host/verify-signature algorithm public-key message signature)
(host/now)
```

Signature verification supports `"ed25519"` and `"p256-sha256"`. The P-256
profile accepts an uncompressed SEC1 public key and a 64-byte P1363 signature;
`p256-jwk-sec1` converts only a strict public verification JWK. Invalid
signatures return `false`; malformed keys, values, or unsupported algorithms
fail the host call.

`canonical-value-digest` hashes the canonical standalone HTA1 frame, not JSON
or printed EDN. Use it when a portable store protocol binds an opaque value to
its exact digest.

## Policy remains in HAL

A handler should perform the complete application-specific sequence:

1. Parse and validate the application's closed credential or signed-request envelope.
2. Reconstruct the exact application-defined signing input.
3. Resolve trusted public-key or secret material through an installed provider.
4. Verify freshness, signature, nonce, idempotency, and revocation according to the application's protocol.
5. Authorize the requested semantic operation before invoking storage or other effects.

Hoplite does not define those semantic fields or their precedence. In
particular, a valid signature is not proof that a nonce was durably consumed,
and a request-body or response-source handle is never an identity credential.

## Secrets and persistence

`hoplite.host/secret` fails closed in the default build because no secret
provider is installed. A distribution may install a secret, key, or durable
store provider, but its paths, credentials, limits, and driver choice must come
from trusted startup configuration. Portable request values must not select
them.

The `legacy-management` Cargo feature remains only for migration validation. It
is disabled in release builds and is not a supported account or session system
for new applications.

See [Host capabilities](/concepts/host-capabilities/),
[Data-plane providers](/concepts/data-plane-providers/), and
[`hoplite.host`](/reference/hoplite-host/) for the exact boundaries.

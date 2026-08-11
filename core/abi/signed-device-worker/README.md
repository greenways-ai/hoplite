# Hoplite signed-device worker ingress

> **Migration/conformance status:** the direct signed-route ingress was removed
> from the provider-neutral runtime. This crate is excluded from the current
> core workspace and is not linked into published release builds. Current
> applications authenticate and authorize inside HAL handlers.

The retained implementation binds the closed `hoplite-signed-device/0-alpha` request to a trusted
Hoplite route, verifies it with the installed Ed25519 key set, atomically
admits its nonce and idempotency key, and returns one bounded application
identity projection.

The wire carries only the signature profile, declared content digest,
timestamp, nonce, idempotency key, key ID, and signature. The actual HTTP
method, target, and authority come from the request exchange. The operation,
application, namespace, and collection come from the selected route. A client
cannot widen those values with headers.

Trusted worker configuration is selected only through:

```text
HOPLITE_SIGNED_DEVICE_KEYS_PATH
HOPLITE_SIGNED_DEVICE_REPLAY_PATH
```

The key document is `hoplite-signed-device-keys/0-alpha`. It contains public keys,
key lifecycle windows, freshness limits, and allowlisted application claims.
It cannot contain private keys, database paths, provider names, or application
state.

The Hara-facing projection excludes signatures, public keys, provider
identity, nonce, idempotency key, credentials, SQL, and filesystem paths.

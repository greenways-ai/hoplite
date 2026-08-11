# Hoplite signed-device Ed25519 provider

> **Migration/conformance status:** this crate is excluded from the current
> Hoplite core workspace and is not linked into published release builds.
> Application authentication is currently owned by HAL policy using generic
> host cryptography. Retained contracts must not be read as active route
> configuration.

This crate is the first concrete provider for the application-neutral
`SignedDeviceProvider` contract in `hoplite-data-plane-abi`.

It owns only trusted host mechanics:

- configured Ed25519 public-key lookup;
- exact `hoplite-signed-device/0-alpha` verification;
- bounded past/future timestamp policy;
- key activation, expiry and revocation checks; and
- construction of an internal signed-device principal.

The public application adapter in `hoplite-data-plane-abi` then removes the
provider identity and non-allowlisted claims before a Hara handler can observe
the verified identity.

## Deliberate boundary

Signature verification and replay admission are separate boundaries. This
crate does **not** claim a nonce has been consumed and does not persist an
idempotency result. The next #73 slice adds one atomic, application-neutral
SQLite admission ledger after verification and before handler invocation.

No request can select a public key, provider package, algorithm, database path,
clock source or freshness limit. Those remain trusted installation inputs.

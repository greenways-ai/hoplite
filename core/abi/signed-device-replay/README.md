# Hoplite signed-device replay admission

This crate supplies the application-neutral durable admission law used after a
Hoplite signed application request has been cryptographically verified.

It binds one exact `hoplite-signed-device/2` signing input to:

- the verified application subject, device and key;
- the application, namespace, collection and operation;
- the content digest and request timestamp;
- the nonce and idempotency key; and
- one server-recorded admission time.

The signature bytes are never persisted. The request fingerprint is SHA-256 of
the exact versioned signing input, which already excludes the signature.

## Atomic law

For one subject/device/application scope:

- the first valid idempotency key is `applied`;
- the same key with the same exact request returns the original evidence as
  `replayed`;
- the same key with different request bytes is an idempotency collision; and
- reusing a nonce under the same subject, device and signing key is rejected.

The in-memory and SQLite stores implement the same `ReplayStore` contract.
SQLite uses one immediate transaction, a primary idempotency key and a unique
nonce key, so concurrent admissions resolve to one applied row and one exact
replay.

## Boundary

The store receives only closed verified request metadata. It does not receive
or persist:

- signature bytes or private keys;
- bearer credentials or management identity;
- Hara values, request-body handles or response-source handles;
- application state or Tahto records; or
- caller-selected database paths inside portable values.

Database paths and retention policy remain trusted host configuration. The
initial profile retains admissions indefinitely; compaction is intentionally
absent until a reviewed retention policy is installed.

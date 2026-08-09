# Hoplite generic value store

This crate defines the dependency-free mechanical contract beneath the Hara
service:

```text
hoplite.store
```

It does not define an application state model. In particular, it has no
knowledge of Tahto objects, graphs, transactions, authorization, receipt fields
or recovery policy.

## Boundary

The Hara client supplies canonical HTA spans for an opaque value and receipt.
A host adapter must preserve those exact spans and use a trusted SHA-256
implementation to construct `CanonicalValue`:

```rust
let value = CanonicalValue::verify(
    canonical_value_bytes,
    claimed_digest,
    &sha256_provider,
    limits,
)?;
```

`DigestVerifier` is intentionally a dependency-inversion boundary. This crate
has no cryptographic dependency, while a production adapter cannot construct a
`CanonicalValue` without recomputing its digest. The test verifier is only a
deterministic conformance fixture; it is not cryptography.

## Mechanical operations

`OpaqueValueStore` exposes:

```text
load
initialize
compare_and_swap
receipt
```

The in-memory implementation enforces:

- canonical lowercase `sha256:<64 hex digits>` identifiers;
- non-empty bounded value and receipt spans;
- revisions within the signed 64-bit persistence range;
- a new revision exactly one greater than the expected revision;
- exact, idempotent initialization;
- stale-writer rejection;
- atomic value and receipt publication;
- exact replay before the stale-revision check;
- receipt-key collision rejection when any committed input differs;
- before-commit failure with no visible mutation;
- after-commit lost-result recovery by load, receipt lookup or exact retry;
- thread-safe single-winner compare-and-swap.

Replay is bound to all mechanically relevant input:

```text
expected revision
new revision
canonical value bytes and digest
receipt key
opaque receipt bytes
```

A receipt key can therefore never be reused to substitute another value.

## Application semantics stay above this crate

The store does not interpret:

- the opaque value;
- the opaque receipt;
- why a receipt key was chosen;
- whether `applied` or `replayed` is a business success;
- how an application recovers or merges state.

Tahto's `.hal` client validates those meanings before and after calling
`hoplite.store`. Other Hara applications can use the same mechanics with different
state and receipt schemas.

## Next slices

1. Decode the `hoplite.store-request/1` frame while preserving the nested canonical
   value and receipt spans.
2. Adapt the reviewed SQLite transaction mechanics to `OpaqueValueStore`.
3. Register the provider as `hoplite.store` through Hoplite's request-scoped host
   provider ABI.
4. Run the same HAL conformance against this in-memory driver and SQLite.
5. Remove the transitional Tahto-specific native migration source after restart
   and fault parity is proven.

Tracked by [Hoplite issue #45](https://github.com/greenways-ai/hoplite/issues/45).

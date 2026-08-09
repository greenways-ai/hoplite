# Hoplite SQLite value store

This crate implements `hoplite-value-store::OpaqueValueStore` using SQLite. It
is an application-neutral production driver beneath the Hara service:

```text
hoplite.store
```

It contains no Tahto state model, object graph, transaction plan, receipt
schema, authorization rule, or recovery policy.

## Trusted installation boundary

The database path and limits are constructor inputs supplied by trusted Hoplite
configuration:

```rust
let store = SqliteValueStore::open(configured_path, configured_limits)?;
```

Neither value can be selected by a HAL request. The eventual host adapter will
only decode canonical request fields and invoke an already-installed store.

## Durability profile

Each connection enables:

```text
foreign_keys = ON
journal_mode = WAL
synchronous = FULL
temp_store = MEMORY
busy timeout = 5 seconds
```

The versioned strict schema stores:

```text
one current opaque canonical value
zero or more exact compare-and-swap receipts
```

A receipt row binds:

```text
receipt key
expected revision
new revision
exact canonical value bytes
value digest
exact opaque receipt bytes
```

The value and receipt are published in one `BEGIN IMMEDIATE` transaction. A
receipt insert failure rolls back the snapshot update.

## Digest and corruption checks

`Sha256Verifier` recomputes SHA-256 over every value entering or leaving the
driver. This is intentionally stricter than trusting a `CanonicalValue` that a
caller may have constructed with another verifier.

Open and explicit verification check:

- exact supported `user_version`;
- SQLite `quick_check`;
- absence of receipts without a snapshot;
- positive one-step receipt revisions;
- no receipt revision beyond the current snapshot;
- real SHA-256 for snapshot and receipt-bound value bytes;
- configured value and receipt limits;
- equality between the current snapshot and a receipt at the current revision.

Unsupported or unversioned existing schemas are rejected without rewriting
them.

## Replay and concurrency

Receipt lookup precedes the stale-revision check. An exact retry therefore
returns `replayed` even after later commits. Reusing a receipt key with any
different revision, value, digest, or receipt bytes returns a collision.

`BEGIN IMMEDIATE`, the snapshot revision predicate, and the unique receipt
revision ensure that multiple connections cannot both publish the same next
revision. Losing writers receive the current revision as a stale-write error.

## Conformance

The suite covers:

- schema creation, pragmas, reopen and restart;
- exact initialization and conflicting initialization;
- atomic CAS and receipt publication;
- exact replay after later commits;
- stale writers across two connections;
- receipt-key and revision collisions;
- real SHA-256 enforcement against an adversarial verifier;
- transaction rollback when receipt insertion fails;
- stored-value corruption detection;
- unsupported schema versions;
- orphan receipt rejection;
- exact nested canonical value and receipt spans after restart.

The next slice is the request-scoped Hoplite provider adapter that preserves the
nested HTA spans and registers this installed driver as `hoplite.store`.

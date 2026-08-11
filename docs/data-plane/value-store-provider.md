# Generic `hoplite.store` protocol adapter

`hoplite-value-store-provider` is the application-neutral boundary between a Hara host call and an `OpaqueValueStore` driver.

It accepts one canonical argument vector containing one closed `hoplite.store-request/0-alpha` map and supports:

```text
load
initialize
compare-and-swap
receipt
```

The host operation and request operation must match exactly. Every operation has an exact field set. Application values and receipts are never decoded into Tahto or another domain schema; the adapter copies their exact nested `HTA0` spans into the value-store contract.

The adapter returns only canonical `hoplite.store-result/0-alpha` frames. Absent load or receipt lookup is represented by `nil`. Receipt lookup is mechanically reported as `replayed`; application HAL decides what replay means.

## Ownership

The adapter owns no database path, driver selection, credentials, application authorization, transaction meaning, or recovery policy. A trusted Hoplite installation constructs it with:

- an `OpaqueValueStore` implementation;
- a cryptographic digest verifier; and
- fixed value/receipt limits.

The same protocol adapter is intended to run unchanged over the in-memory and SQLite drivers.

## Errors

Protocol, HTA, revision, digest, collision, stale-writer, fault, and driver failures retain stable application-neutral error codes. A later portable provider binding converts those errors into a closed failure frame without exposing SQL text, storage paths, credentials, or opaque application bytes.

# `hoplite.store` native provider boundary

This package exports the canonical C ABI for a separately installed durable
`hoplite.store` implementation. The current driver is SQLite.

The ABI owns canonical request decoding, bounded opaque value and receipt
spans, atomic revision compare-and-swap, receipt replay, digest validation and
provider result ownership. It does not own application state models,
authorization, transition policy, idempotency meaning or recovery policy.

## Process composition

A store-only distribution links this archive with
`core/nginx/hoplite_store_host_provider.c` and registers exactly one
`hoplite.store` service from trusted process configuration:

- `HOPLITE_STORE_PATH`;
- `HOPLITE_STORE_MAX_VALUE_BYTES`;
- `HOPLITE_STORE_MAX_RECEIPT_BYTES`.

The older `hoplite_value_store_host_provider_*` entry points remain a
compatibility aggregator for distributions that install both `hoplite.store`
and `hoplite.value`. Store-only distributions do not link the value provider or
that aggregate lifecycle.

Paths, limits, provider identity, SQL and credentials are distribution inputs.
They must never enter request or HAL values.

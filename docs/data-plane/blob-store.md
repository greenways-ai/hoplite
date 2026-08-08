# Generic `hara.blob` mechanics

`hoplite-blob-store` defines the application-neutral staged-blob lifecycle beneath the `hara.blob` service:

```text
staging/open
staging/append-from-source
staging/abort
staging/verify-commit
object/open-source
```

It contains no Tahto upload, namespace, quota, authorization, graph, manifest, receipt or recovery semantics.

## Identity

A staging key is a bounded logical resume key. It is never interpreted as a filesystem path. Physical staging and object locations are derived by installed drivers from trusted configuration.

An object is identified by a canonical lowercase SHA-256 digest and exact size. The driver recomputes the digest over the bytes actually staged before commit.

## Ingress source

Append consumes one generic bounded `ByteSource`. The source is supplied by the host adapter, which may bind it to a work-scoped Nginx request body. The storage contract knows nothing about Nginx or numeric body handles.

Append requires the exact current offset and exact declared source length. Short, long, failing or unclosed sources fail before the staged object is advanced.

## Egress source

Opening an object range returns a generic response source with a fixed declared length, bounded sequential reads and exactly-once close. Hoplite's Nginx adapter later owns source-handle registration, output backpressure and cancellation.

## Reference driver

The in-memory implementation is deterministic conformance infrastructure. It intentionally materializes fixture bytes and is not the production object-store driver. Production filesystem or object-storage drivers must preserve the same laws while streaming bytes directly.

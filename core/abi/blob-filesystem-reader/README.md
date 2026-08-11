# Hoplite filesystem object reader

`hoplite-blob-filesystem-reader` is the application-neutral read-only boundary
for immutable objects stored by a filesystem `hoplite.blob` provider.

It is not a Hara service and it does not define another object store. It owns the
single reusable implementation of the installed filesystem read law:

```text
canonical digest + positive maximum
  -> trusted objects/sha256 lookup
  -> shared store.lock
  -> exact HBO0 metadata
  -> bounded actual read
  -> actual byte-length agreement
  -> actual SHA-256 verification
  -> verified bounded bytes
```

## Public Rust boundary

```rust
let reader = FilesystemObjectReader::open(
    configured_root,
    Limits {
        max_object_bytes: 1024 * 1024,
        max_media_type_bytes: 256,
        io_chunk_bytes: 64 * 1024,
    },
)?;

let object = reader.read_verified(digest, request_maximum)?;
```

The result carries only the requested digest and exact verified bytes. It does
not carry a path, file descriptor, provider identity, source handle, schema,
application coordinate or decoded value.

## Consumers

`hoplite.value` consumes this reader and adds only:

```text
closed request validation
  -> canonical HTA decoding
  -> closed hoplite.value-result/0-alpha
```

The filesystem blob driver remains responsible for staging and immutable
installation. Its `object/open-source` path delegates whole-object verification
and bounded range-source construction to this package. `hoplite.value` delegates
bounded value materialization to the same backend, leaving one authoritative
filesystem object-read implementation.

## Failure boundary

Read failures are intentionally mechanical and closed:

```text
Missing
Maximum
Digest
Provider
```

Each service adapter translates those classes into its own public result
vocabulary. Filesystem paths and operating-system details never cross the
portable Hara boundary.

## Non-goals

- staging or committing objects;
- response-source registration or streaming;
- canonical HTA decoding;
- Tahto authorization or semantic admission;
- mutable state or `hoplite.store`;
- request-selected roots, providers or drivers.

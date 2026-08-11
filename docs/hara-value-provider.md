# Installed `hoplite.value` provider

A provider-enabled distribution may install one application-neutral
canonical-value verification service:

```text
service    hoplite.value
operation  object/verify-hta
```

The service is a read-only projection over immutable objects owned by
`hoplite.blob`. It does not create a second object copy, filesystem reader,
decoded-value cache, semantic index, schema registry or application store.

## Trusted worker configuration

```text
HOPLITE_VALUE_PROVIDER=filesystem
HOPLITE_VALUE_ROOT=/var/lib/hoplite/blob
HOPLITE_VALUE_MAX_BYTES=1048576
```

All three values are startup-only installation authority. If none is present,
the service is not installed. If only some are present, the provider type is
unknown, the root cannot be opened, or the ceiling is invalid, worker startup
fails closed.

Portable Hara requests contain only:

```clojure
{:protocol "hoplite.value-request/1"
 :operation "object/verify-hta"
 :digest "sha256:..."
 :max-bytes 1048576}
```

A request cannot select the provider, root, path, driver, decoder, package,
schema, application, credential, command or remote location.

## Worker lifecycle

During trusted worker startup Hoplite:

1. opens the shared read-only filesystem object provider against the configured
   blob root;
2. validates the installed maximum and fixed internal read/media-type bounds;
3. materializes the value adapter through its stable C ABI;
4. registers exactly one immutable `hoplite.value` service entry;
5. rejects duplicate registration or an ABI mismatch;
6. closes the adapter during worker shutdown.

The service has no request-body or response-source capability. It returns one
owned bounded HTA result through the ordinary host completion path.

## Verification path

```text
registered hoplite.value call
  -> exact closed request
  -> shared blob filesystem reader
       -> digest-derived provider-owned path
       -> shared store.lock
       -> exact HBO1 metadata
       -> bounded actual read
       -> actual size and SHA-256 verification
  -> Hara canonical HTA decoding
  -> closed hoplite.value-result/1
```

The reader never treats metadata size as proof of the bytes. The value adapter
contains no `objects/sha256`, `HBO1`, lock, filesystem or SHA-256 implementation
and never exposes operating-system, path or provider details in portable
results.

## Build pin

`packaging/hara-value-revision` pins the Hara revision that supplies the
canonical decoder used by this adapter. This pin is separate from Hoplite's
runtime/compiler pin because the provider is an isolated static library. The
installed-provider workflow checks out and tests that exact revision before
materialization.

## Object backend lock

`packaging/providers/value/provider-manifest.json` declares only the closed
`hoplite.value` service, protocol, ABI and filesystem-driver compatibility. It
does not declare another object store.

`packaging/providers/value/object-backend-lock.json` binds that service to:

```text
backend package   hoplite-blob-filesystem-reader/0.1.0
container package hoplite-blob-provider/0.1.1
artifact digest   sha256:03c5dea9854cf23b60c7d2638c17712accc7e77eb53db4d15ed0b45327ee8210
```

The object-backend workflow validates the value manifest, validates the
published blob provider lock, downloads and verifies the exact archive, checks
its closed file inventory and byte-compares the reader package in the archive
with the reader compiled into the local value adapter. A package, version or
digest mismatch fails before value-provider materialization.

The generic lock parser is tested with the current stable core toolchain. After
that closed compatibility gate succeeds, the isolated value-provider package is
built and linted with its Rust 1.78 compatibility toolchain and exact pinned Hara
canonical decoder. This split does not change either portable contract.

The backend lock and all compatibility expectations are trusted distribution
inputs. They are not available through `hoplite.value-request/1`, cannot be
selected by application HAL and do not change namespace authorization.

This boundary proves backend identity but does not yet publish a complete
standalone value-provider artifact. The following release slice can materialize
that package from the locked reader and canonical Hara decoder without
reintroducing filesystem ownership into the value adapter.

## Boundary with Tahto

The shared reader proves immutable byte identity. `hoplite.value` proves that
those bytes are one bounded canonical portable Hara value. Tahto remains
responsible for prior namespace authorization, expected object identity, exact
schema-reference binding, local package-root resolution, validator-entry
invocation and semantic mutation. Installing `hoplite.value` does not install or
authorize a specification package.

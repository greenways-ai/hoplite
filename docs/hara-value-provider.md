# Installed `hoplite.value` provider

Hoplite installs one application-neutral canonical-value verification service:

```text
service    hoplite.value
operation  object/verify-hta
```

The installed filesystem provider reuses the immutable object root already owned
by `hoplite.blob`. It does not create a second copy, decoded-value cache, semantic
index, schema registry or application store.

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

1. opens the configured immutable object root;
2. validates the installed maximum and fixed internal read/media-type bounds;
3. materializes the filesystem provider through its stable C ABI;
4. registers exactly one immutable `hoplite.value` service entry;
5. rejects duplicate registration or an ABI mismatch;
6. closes the provider during worker shutdown.

The service has no request-body or response-source capability. It returns one
owned bounded HTA result through the ordinary host completion path.

## Verification path

```text
registered hoplite.value call
  -> exact closed request
  -> digest-derived provider-owned path
  -> shared blob-store lock
  -> exact HBO1 metadata
  -> bounded actual read with maximum-plus-one sentinel
  -> actual size and SHA-256 verification
  -> Hara canonical HTA decoding
  -> closed hoplite.value-result/1
```

The provider never treats metadata size as proof of the bytes and never exposes
operating-system, path or provider details in portable results.

## Build pin

`packaging/hara-value-revision` pins the Hara revision that supplies the
canonical decoder used by this provider. This pin is separate from Hoplite's
runtime/compiler pin because the provider is an isolated static library. The
installed-provider workflow checks out and tests that exact revision before
materialization.

## Boundary with Tahto

Hoplite proves canonical byte identity and returns the bounded portable value.
Tahto remains responsible for prior namespace authorization, expected object
identity, exact schema-reference binding, local package-root resolution,
validator-entry invocation and semantic mutation. Installing `hoplite.value` does
not install or authorize a specification package.

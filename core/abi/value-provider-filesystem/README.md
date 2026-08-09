# Hoplite filesystem `hoplite.value` provider core

`hoplite-value-provider-filesystem` implements the application-neutral installed
provider mechanics for:

```text
service    hoplite.value
operation  object/verify-hta
```

It reuses the immutable object layout and trusted filesystem root owned by the
existing `hoplite.blob` provider. It does not create a second object copy, semantic
cache, Tahto store, or application-specific index.

## Verification path

```text
closed hoplite.value-request/1
  -> canonical lowercase SHA-256 identity
  -> trusted digest-derived .meta and .blob paths
  -> shared store.lock
  -> metadata ceiling check
  -> bounded actual read with maximum-plus-one sentinel
  -> actual byte-length check
  -> SHA-256 over actual bytes
  -> hara_hta::decode_canonical(bytes, max_bytes)
  -> closed hoplite.value-result/1
```

The request can contain only:

```clojure
{:protocol "hoplite.value-request/1"
 :operation "object/verify-hta"
 :digest "sha256:..."
 :max-bytes 1048576}
```

It cannot select a root, path, driver, provider, URL, credential, source handle,
decoder, application, namespace, schema, package, or command.

## Authority boundary

Hara owns the request/result profiles, portable `hara.hta/1` value model,
canonical decoder and stable generic failure codes.

Hoplite owns trusted-root selection, exact object lookup, bounded file reads,
short/excess-read detection, actual SHA-256 verification, provider lifecycle and
native-to-portable HTA composition.

Tahto remains responsible for namespace authorization before dispatch, expected
object-size agreement, exact schema-reference binding, installed specification
validation and all semantic mutation.

## Storage compatibility

The provider expects the existing restart-safe blob layout:

```text
<root>/
├── store.lock
└── objects/sha256/
    └── <first-two-hex>/
        ├── <remaining-hex>.meta
        └── <remaining-hex>.blob
```

The root, `objects`, `sha256`, digest-prefix directory, metadata file, data file
and lock must all be real installation-owned paths. Request data never becomes a
filesystem path.

Object metadata is checked for exact `HBO1` closure, digest identity, bounded
media type and declared size. The declared size permits an early conservative
maximum rejection but never replaces the bounded actual read. The provider reads
at most `max-bytes + 1`, compares actual and metadata lengths, and recomputes the
digest before decoding.

## Failure normalization

A valid digest-bound request receives only one of Hara's closed generic codes:

```text
hoplite.value/object-missing
hoplite.value/maximum-exceeded
hoplite.value/digest-mismatch
hoplite.value/hta-invalid
hoplite.value/hta-noncanonical
hoplite.value/value-unsupported
hoplite.value/provider-failure
```

No result contains an operating-system error, root, path, driver, provider or
native detail.

## Hara dependency

The crate consumes `hara-hta` from a sibling immutable Hara checkout. The
dedicated workflow pins the merge revision of `hara-lang/hara#464`, which added
`decode_canonical`. The broader Hoplite runtime pin is intentionally not moved in
this provider-core slice; installed host registration and dependency-train
reconciliation remain Hoplite #82.

## Conformance

The Rust 1.78 suite covers:

- valid canonical portable values and exact closed results;
- exact maximum, maximum-plus-one during actual I/O and installed ceilings;
- missing objects and on-disk digest tampering;
- malformed, truncated and trailing HTA;
- decodable but noncanonical map ordering;
- runtime-only HTA tags;
- short and excess actual reads relative to object metadata;
- provider restart against the same object root;
- rejection of request path/provider authority; and
- coexistence with the existing filesystem blob/source provider.

Registration under the production Hoplite host registry is deliberately left to
Hoplite #82.

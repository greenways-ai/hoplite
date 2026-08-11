# Hoplite filesystem `hoplite.value` adapter

`hoplite-value-provider-filesystem` implements the application-neutral installed
service adapter for:

```text
service    hoplite.value
operation  object/verify-hta
```

It does not implement another object store or filesystem read law. Immutable
object lookup, `HBO0` metadata validation, lock coordination, bounded reads and
actual SHA-256 verification are supplied by the shared
`hoplite-blob-filesystem-reader` package over the object root owned by
`hoplite.blob`.

## Verification path

```text
closed hoplite.value-request/0-alpha
  -> shared verified object reader
  -> exact digest and maximum agreement
  -> hara_hta::decode_canonical(bytes, max-bytes)
  -> closed hoplite.value-result/0-alpha
```

The request can contain only:

```clojure
{:protocol "hoplite.value-request/0-alpha"
 :operation "object/verify-hta"
 :digest "sha256:..."
 :max-bytes 1048576}
```

It cannot select a root, path, driver, provider, URL, credential, source handle,
decoder, application, namespace, schema, package or command.

## Authority boundary

Hara owns the request/result profiles, portable `hara.hta/0-alpha` value model,
canonical decoder and stable generic value-verification failure codes.

The shared blob filesystem reader owns:

- the installation-selected immutable object root;
- digest-derived lookup under `objects/sha256`;
- the shared `store.lock` read boundary;
- exact `HBO0` metadata validation;
- bounded actual reads and short/excess-read detection;
- actual-byte SHA-256 verification.

This adapter owns only:

- exact `hoplite.value-request/0-alpha` validation;
- mandatory request and installation ceilings;
- translation of mechanical reader failures into `hoplite.value/*` results;
- canonical HTA decoding and portable-value classification;
- exact `hoplite.value-result/0-alpha` construction.

Tahto remains responsible for namespace authorization before dispatch, expected
object-size agreement, exact schema-reference binding, installed specification
validation and all semantic mutation.

## Result boundary

A verified object returns the decoded portable value:

```clojure
{:protocol "hoplite.value-result/0-alpha"
 :operation "object/verify-hta"
 :verified true
 :digest "sha256:..."
 :byte-length 512
 :profile "hara.hta/0-alpha"
 :value decoded-portable-value}
```

A valid digest-bound failure returns one closed code:

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

The adapter consumes `hara-hta` from a sibling immutable Hara checkout. The
dedicated workflow pins the merge revision of `hara-lang/hara#464`, which added
`decode_canonical`. The broader Hoplite runtime pin remains independent because
the installed adapter is built as a standalone static library.

## Conformance

The Rust 1.78 suites cover:

- the shared reader independently of canonical decoding;
- valid canonical portable values and exact closed results;
- injected non-filesystem readers and stable failure translation;
- exact maximum, maximum-plus-one actual I/O and installed ceilings;
- missing, incomplete and tampered filesystem objects;
- malformed, truncated and trailing HTA;
- decodable but noncanonical map ordering;
- runtime-only HTA tags;
- restart against the same blob-owned object root;
- absence of filesystem layout, lock and SHA-256 implementation from the value
  adapter; and
- coexistence with the existing filesystem blob/source provider.

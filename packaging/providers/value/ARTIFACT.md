# `hoplite.value` provider source artifact

The filesystem implementation of `hoplite.value` is distributed as a
**target-independent deterministic source artifact**. It remains separate from
the generic Hoplite executable, Nginx module and production image.

## Contents

The artifact contains only:

- the closed `hoplite.value` provider compatibility manifest;
- the closed object-backend lock;
- the published blob-provider lock and manifest used by that binding;
- the shared immutable filesystem reader and its generic digest contract;
- the bounded canonical-value adapter and C provider boundary;
- the canonical provider HTA frame dependency;
- the exact Hara ABI and canonical HTA decoder source;
- every standalone Cargo manifest and lock needed to build the FFI package;
- both source revisions, package version, licenses and a SHA-256 file inventory.

It contains no Nginx lifecycle module, object root, credentials, Tahto package,
schema, namespace policy or mutable `hoplite.store` implementation.

## Repository layout

The artifact preserves the source relationship required by the reviewed value
adapter:

```text
hoplite-value-provider-VERSION/
  hoplite/core/abi/...
  hara/core/rust/abi
  hara/core/rust/hta-codec
```

This lets the unchanged standalone Cargo manifests resolve the exact Hara
canonical decoder without substituting a registry package or rewriting source
paths during publication.

## Determinism

`packaging/scripts/build-value-provider-artifact.sh` accepts one exact Hoplite
commit and one exact Hara commit. It requires the Hara commit to match
`packaging/hara-value-revision` at the selected Hoplite revision, archives a
closed source inventory from both repositories, normalizes modes and metadata,
writes a sorted per-file SHA-256 inventory and creates a normalized
gzip-compressed ustar archive.

The artifact workflow builds the same two revisions twice and requires
byte-for-byte archive and checksum identity.

## Backend identity

Before building, the workflow:

1. validates the closed `hoplite.value` provider manifest;
2. validates `hoplite.object-backend-lock/v1` against the published blob
   provider lock;
3. downloads `hoplite-blob-provider-v0.1.1`;
4. verifies its exact archive SHA-256 and internal inventory;
5. byte-compares the reader crate in that archive with the reader copied into
   the value-provider artifact.

The value artifact therefore records and carries the same physical reader bytes
used by the pinned blob provider. It does not merely repeat a compatible name.

## Verification

`packaging/scripts/verify-value-provider-artifact.sh` fails closed on:

- a missing artifact or invalid/mismatched archive digest;
- unsafe, duplicate, unsupported or multi-root archive paths;
- links and non-regular filesystem entries;
- an incompatible package format or package/root mismatch;
- invalid Hoplite or Hara source revisions;
- missing provider, backend, reader, decoder or FFI files;
- malformed, duplicate or mismatched file inventory entries;
- files present outside the closed inventory.

After extraction, the workflow revalidates the provider/backend contracts and
builds the exact FFI package with Rust 1.78 using the embedded Hara decoder.

## Publication

The initial source release is:

```text
hoplite-value-provider-v0.1.0
hoplite-value-provider-0.1.0.tar.gz
```

The source-tree provider manifest keeps `artifact.digest` null because an
archive cannot contain its own digest. A follow-up pin PR will add an external
published manifest and provider lock containing the actual merge-revision
archive SHA-256 before any production distribution consumes the value package.

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

## Current pinned production package `0.1.0`

```text
release tag       hoplite-value-provider-v0.1.0
Hoplite revision  bbc42008a44223a977a84ffedb8f2262ba06f1ba
Hara revision     d2a2e376917131cfb8954ac156db3a5e174f2c1a
archive            hoplite-value-provider-0.1.0.tar.gz
archive SHA-256   47e96af3768621b25ef448004795ce9ecbdca091cfa31910308009156ed89e4f
object backend    03c5dea9854cf23b60c7d2638c17712accc7e77eb53db4d15ed0b45327ee8210
```

`provider-manifest.published.json` binds the value archive digest.
`provider-lock.json` additionally binds the provider version, exact Hoplite
source revision, release repository and tag, asset name and media type.

The archive itself records and verifies the exact Hara decoder revision and the
closed object-backend lock. The permanent lock workflow therefore proves four
linked identities before any value package can be consumed:

```text
value artifact bytes
Hoplite source revision
Hara canonical decoder revision
blob object-backend artifact bytes
```

The source-tree provider manifest keeps `artifact.digest` null because an
archive cannot contain its own digest. Production composition must consume the
published manifest and provider lock, verify the archive before extraction, and
revalidate the embedded decoder and backend identities.

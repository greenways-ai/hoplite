# `hoplite.blob` provider source artifact

The filesystem implementation of `hoplite.blob` is distributed as a
**target-independent deterministic source artifact**. It is not part of the
generic Hoplite executable, Nginx module or production image.

## Contents

The artifact contains only:

- the application-neutral blob-store contract;
- the filesystem driver;
- the canonical `hoplite.blob` protocol adapter;
- the work-scoped C provider boundary;
- the canonical provider HTA dependency;
- each crate's exact Cargo manifest and lock;
- the closed source-tree provider compatibility manifest;
- license, source revision, package version and a SHA-256 file inventory.

The distribution-owned Nginx lifecycle module is deliberately not included. It
remains part of the product composition that selects and links the provider.

## Determinism

`packaging/scripts/build-blob-provider-artifact.sh` resolves one exact Git
commit, copies the closed source inventory, normalizes file modes, writes a
sorted per-file SHA-256 inventory and creates a normalized gzip-compressed
ustar archive.

The artifact workflow builds the same revision twice and requires byte-for-byte
identity before uploading either result.

## Verification

`packaging/scripts/verify-blob-provider-artifact.sh` fails closed on:

- a missing artifact;
- an invalid or mismatched archive digest;
- absolute, parent-relative, duplicate or unsupported paths;
- links or non-regular filesystem entries;
- multiple archive roots;
- an incompatible package format or version/root mismatch;
- a missing required contract or provider crate;
- malformed, duplicate or mismatched file inventory entries;
- files that are present outside the closed inventory.

After extraction, `hoplite-provider-manifest` validates the portable contract,
ABI and driver declarations. The package is tested with Rust 1.78 and built
independently from the Hoplite core workspace.

## Published package `0.1.0`

```text
release tag      hoplite-blob-provider-v0.1.0
source revision  5d5b98b379bee49db54f79492441d83bd192930b
archive           hoplite-blob-provider-0.1.0.tar.gz
archive SHA-256  5a8b3735e7b8c147b2879647455db7d1769f26964e4f8c78432935adb040e362
```

The source-tree `provider-manifest.json` keeps `artifact.digest` null because an
artifact cannot contain its own byte digest. The external
`provider-manifest.published.json` binds the archive digest, while
`provider-lock.json` additionally binds the version, source revision, release
tag, repository, asset name and media type.

`hoplite-provider-lock` validates both documents as closed JSON contracts,
requires their digests to match, rejects incompatible release identities and
emits only validated shell-safe fields for trusted distribution composition.

## Production composition

`Dockerfile.blob-provider` no longer compiles the provider from the implicit
Hoplite source tree. It:

1. validates the published manifest and lock;
2. downloads the exact versioned release asset;
3. verifies its SHA-256 before extraction;
4. verifies its closed internal file inventory;
5. checks the extracted source revision and package version;
6. builds the provider from `/src/provider`;
7. links it through the distribution-owned Nginx lifecycle module.

The release host's mutability settings are not a correctness dependency. The
reviewed lock, exact digest, extracted inventory and compatibility checks remain
mandatory and fail closed.

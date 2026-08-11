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
- the closed provider compatibility manifest;
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

After extraction, the existing `hoplite-provider-manifest` command validates the
portable contract, ABI and driver declarations. The package is then tested with
Rust 1.78 and built independently from the Hoplite core workspace.

## Publication boundary

The source-tree provider manifest keeps `artifact.digest` null. A published
manifest and distribution lock will bind the exact uploaded archive digest,
source revision and immutable download identity. Production composition must
verify those bytes before extraction or compilation.

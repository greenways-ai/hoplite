# Hoplite core and migration boundary

This document defines what the Hoplite alpha repository builds as the default
product and how historical product implementations are quarantined while they
are extracted. The machine-readable inventory is
[`core-boundary.json`](core-boundary.json), identified as
`hoplite.core-boundary/0-alpha`.

The boundary is intentionally stricter than repository layout. Source can remain
available as migration input without becoming part of the default package,
compiled command set, dependency graph, production image, or public API.

## Default product

A normal Cargo build exposes exactly two binary targets:

- `hoplite`, the development and control command;
- `hoplite-server`, the source-free production serving command.

The default feature set is `bytecode-vm`, `native-jit`, and `cli-host`. It owns
application composition, package activation, route preparation, the Hara runtime,
Nginx integration, request/response transport, generic host services, production
bootstrap, verification, and development tooling.

The generic `hoplite.host` boundary includes bounded Base64 decoding, secure
randomness, hashing, canonical HTA digests, P-256 JWK conversion, and Ed25519 and
P-256 signature verification. The cryptographic libraries needed to implement
those provider-independent operations are therefore core runtime dependencies,
not application-authentication product dependencies.

Cargo automatic binary discovery is disabled. Adding a file under
`core/src/bin` therefore cannot silently add another shipped program. Every
binary target must be declared deliberately and classified in both the public
surface registry and the core-boundary inventory.

The ordinary `make setup` path fetches only the core workspace and runtime. It
does not fetch or build the standalone blob, value, store, or authentication
products.

## Opt-in migration and evidence features

Four non-default features make retained source explicit:

| Feature | Purpose |
| --- | --- |
| `internal-evidence` | Enables measurement programs that are useful to Hoplite development but are not supported user commands. |
| `legacy-management` | Enables the historical application-authentication and management implementation while it is extracted. |
| `legacy-provider-products` | Enables tiny Cargo wrappers for provider manifest and lock implementations physically quarantined beneath `migration/provider-products`. |
| `legacy-value-contract` | Enables the physically quarantined historical `hoplite.value` HAL contract only for migration conformance. |

The authentication-store ABI, the external SQLite store implementation, and
Rust SQLite integration are optional dependencies activated only by
`legacy-management`. They are not direct dependencies of the default Hoplite
product. Generic host cryptography remains available without enabling historical
application policy or database storage.

The legacy features are temporary migration seams, not extension points and not
release promises. Provider manifest/lock validators and their CLI implementations
are physically outside `core/src`; only target-name wrappers remain for the
documented compatibility window. New generic runtime code must not depend on them. Ordinary
resource registration and application-bundle construction exclude `hoplite.value`;
only the explicit `legacy-value-contract` feature can restore it for focused
compatibility testing.

## Inventory statuses

| Status | Meaning |
| --- | --- |
| `public` | Supported Hoplite product or embedding surface. |
| `core` | Required generic runtime, build, packaging, or production evidence. |
| `generic-interface` | Replaceable host/data-plane interface owned by Hoplite rather than one provider product. |
| `reference-conformance` | Narrow fixture retained because it tests a generic invariant; it must not pull a product implementation into the default build. |
| `development` | Development-only tooling outside the source-free serving process. |
| `distribution` | Generic installation or reviewed-release packaging. |
| `internal-evidence` | Scheduled/manual measurements and compatibility helpers, not shipped commands. |
| `migration-only` | Historical policy, provider, storage, release, or product implementation awaiting extraction or retirement. |

The required workspace test inventories every Rust source under `core/src`, every
top-level package under `core/abi`, every provider-product packaging directory,
every physically quarantined top-level migration product, and every packaging
script. A new path without an explicit status fails CI.

## Direct default-product evidence

`packaging/scripts/verify-default-product.sh` uses a clean Cargo target directory
to build all default binary targets. It requires the resulting executable set to
be exactly `hoplite` and `hoplite-server`.

The same check reads Cargo's resolved direct dependency tree. It positively
requires the Hara runtime, Hoplite application-bundle library, and the generic
host cryptography dependencies. It rejects the migration-only authentication
store ABI, external SQLite store product, and direct SQLite database dependency
from the default package. This is dependency and build evidence, not a
source-text approximation.

The production image remains independently checked by
`assert-production-image.sh`; only `hoplite-server` and built `.hoplite` output
cross into the final image.

## Retained migration material

The current repository still contains substantial historical implementation
work:

- application authentication, policy, management, replay, and signed-device
  code;
- blob, value, and store contracts and filesystem/SQLite implementations;
- product-specific native adapters and Nginx modules;
- provider manifests, locks, release artifacts, and artifact build/verification
  scripts;
- a blob-store conformance package whose useful corruption, cancellation, and
  cleanup invariants must be projected onto generic interfaces before removal.

Keeping this material visible avoids throwing away tested invariants. Its
`migration-only` status prevents it from dictating Hoplite's public architecture.

## Extraction law

An extraction change must do all of the following:

1. identify the destination repository or explicitly retire the product;
2. preserve any generic transport, boundedness, corruption, cancellation,
   disconnect, and exactly-once cleanup invariant at the generic interface;
3. remove the migrated path from `core-boundary.json` in the same change that
   removes it from this repository;
4. keep the default binary and direct-dependency evidence green;
5. avoid adding replacement abstractions solely to preserve historical product
   structure.

The intended order is application policy/management first, provider release and
lock tooling second, concrete blob/value/store implementations third, and the
remaining temporary legacy features last. Generic data-plane and host-provider
interfaces stay in Hoplite when they can be exercised without a product
implementation.

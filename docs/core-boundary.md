# Hoplite core boundary

Hoplite 0.2.0 is one generic, library-first HTTP runtime. The machine-readable
inventory is [`hoplite.core-boundary/0-alpha`](core-boundary.json).

## Default product

A normal build exposes exactly `hoplite` and `hoplite-server`. The default
features are `bytecode-vm`, `native-jit`, and `cli-host`; `internal-evidence` is
the only opt-in feature and enables scheduled measurement programs that are not
shipped commands.

Core owns application and route composition, worker-local prepared dispatch,
request/response ownership, suspension, streaming/backpressure, HAB0/HBX0
startup, generic host/data-plane ABIs, diagnostics, and development/build tools.
The generic host boundary may implement application-neutral hashing, decoding,
randomness, key conversion, and signature verification without granting an
identity, credential, database, room, or application policy.

## Reviewed retirement

The 0.2.0 boundary retires the historical authentication, account, management,
signed-device, replay, blob, value, store, provider-product, native adapter,
artifact publishing, lock, and migration implementations. They have no
destination repository and no compatibility feature. Their prior source remains
recoverable from repository history before 0.2.0.

The retirement deliberately preserves generic invariants at their owning
interfaces:

- bounded body and response-source reads;
- work-scoped opaque handles and stale-handle rejection;
- cancellation, disconnect, corruption, restart, and exactly-once cleanup;
- immutable worker-local host registration and completion ownership.

Hoplite no longer documents or publishes a provider marketplace, storage
product, provider artifact, authentication realm, or management service.

## Inventory statuses

| Status | Meaning |
| --- | --- |
| `public` | Supported product or embedding surface. |
| `core` | Required generic runtime, build, packaging, or evidence. |
| `generic-interface` | Replaceable application-neutral ABI. |
| `reference-conformance` | Fixture protecting a generic invariant. |
| `development` | Development-only tooling outside production. |
| `distribution` | Generic installation or release packaging. |
| `internal-evidence` | Scheduled/manual evidence, not a shipped command. |

CI inventories every Rust source under `core/src`, Cargo package under
`core/abi`, provider/migration product directory, and packaging script. The
provider and migration inventories must remain empty. Adding an unclassified
path fails the library gate.

`verify-default-product.sh` builds in a clean target directory, requires exactly
the two supported binaries, and inspects the resolved dependency graph. The
production-image check independently proves that only `hoplite-server` and
generated source-free `.hoplite` artifacts cross into the final image.

## Downstream law

Hestia and Greenways OS are ordinary downstream consumers. Hoplite contains no
Hestia profile, room, mandate, approval, or projection type and no Greenways OS
client registry, credential vault, application approval, or node-routing policy.
Their build proofs use Hoplite’s public bundle/embedding interfaces; their
domain policy never enters this repository.

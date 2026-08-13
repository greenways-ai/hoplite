# Legacy provider distribution tooling

This directory contains the historical provider manifest, provider lock, object
backend lock, and provider-set lock validators and their CLI implementations.
They are trusted distribution tooling retained only for extraction and
compatibility. They are not part of the default Hoplite binaries, generic
application model, worker runtime, production image, or required merge gate.

The existing Cargo target names remain as tiny wrappers under `core/src/bin` so
a deliberate build with `legacy-provider-products` can run the exact historical
interfaces during the migration window. Request data and application values
cannot enable these targets or select provider packages, artifact identities,
repositories, tags, paths, roots, credentials, or backends.

## Compatibility window

The wrappers remain for one documented pre-1.0 migration window while provider
release and distribution ownership moves to the extracted products. New generic
Hoplite code must not import these validators. Eventual removal must preserve
closed-schema parsing, bounded input, duplicate-field rejection, exact digest
binding, request-selection exclusion, and negative conformance in the owning
product repositories.

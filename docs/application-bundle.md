# Hoplite application bundle alpha

Production workers boot one **Hoplite application bundle** rather than treating
compiled Hara modules and the route manifest as unrelated inputs.

The current portable document identity is
`hoplite.application-bundle/0-alpha`. Its four-byte outer marker is `HAB0`.
This is an explicitly pre-release contract, not a stable-major compatibility
promise.

## Format

The byte layout is deterministic:

```text
HAB0
sha256(payload)
runtime ABI (u32 little-endian)
sha256(exact apps.hta bytes)
embedded bytecode length (u32 little-endian)
embedded Hara HBX0 bytecode bundle
```

`HAB0` is the Hoplite-owned alpha application envelope. `HBX0` remains the
Hara-owned bundle for namespace-preserving `HBC0` modules. Hoplite does not
duplicate Hara's module table inside its envelope.

The alpha decoder rejects:

- unsupported runtime ABI versions;
- an empty or oversized route manifest;
- an empty, oversized, or non-`HBX0` bytecode payload;
- checksum, manifest-digest, and declared-length mismatches;
- truncation and trailing bytes;
- markers from the pre-reset stable-looking format epoch.

The current limits are 8 MiB for `apps.hta` and 64 MiB for embedded `HBX0`
bytecode.

## Golden and migration fixtures

`core/application-bundle/tests/fixtures/hab0-golden.hex` is the committed exact
byte contract for one small deterministic envelope. The test suite requires the
current encoder to reproduce all 95 bytes and requires the decoder to recover
the exact manifest-bound `HBX0` payload.

`hab1-migration-rejected.hex` preserves the same payload with only the final
outer-marker byte incremented. The current decoder must reject that fixture. A
future alpha epoch therefore cannot become an accidental alias: it must add its
own format identity, decoder, golden bytes, migration note, and explicit
acceptance policy while retaining the old fixture as compatibility evidence.

Golden fixtures are append-only compatibility records. Changing the current
encoder requires a new epoch fixture rather than rewriting the bytes that
represent an already published contract.

## Startup law

The worker validates the complete `HAB0` envelope, then performs a full
bytecode-and-route preflight on an isolated thread. Hara's thread-local protocol
and multimethod state cannot leak from a rejected preflight. Only after that
succeeds does Hoplite construct a fresh staged runtime, load the same `HBX0`
bytes, prepare the same route handlers, and atomically replace the pristine
worker runtime.

A bytecode failure, unknown handler, malformed manifest, checksum mismatch, or
manifest substitution leaves the serving runtime with neither partially loaded
namespaces nor partially prepared routes. Combined application bootstrap is
therefore accepted only on a fresh runtime.

## Compatibility during alpha

The envelope records numeric Hoplite runtime ABI compatibility separately from
its portable document identity. Runtime ABI `4` and exported symbols such as
`hoplite_bootstrap_application_v2` describe native call shapes; they do not
promote the application document out of alpha.

An incompatible change to the envelope, manifest-binding law, or required inner
Hara format must change the alpha epoch or marker and include a migration note.
The current code makes no stable-major application-bundle claim.

The lower-level `hoplite_bootstrap_bytecode`, `hoplite_apps_prepare`, and
byte-oriented `hoplite_bootstrap_application_v1` symbols remain available for
embedding compatibility. Generated Nginx production configuration uses the
bounded file-based `hoplite_bootstrap_application_files_v1` entry point. Runtime
ABI 4 adds `_v2` forms of both calls with ordered
`hoplite.startup-diagnostic/0-alpha` callbacks.

See [Versioning during alpha](versioning.md) for the distinction between package
versions, portable format epochs, and native ABI generations.

## Source-free verification

`hoplite verify [--manifest FILE] [PROJECT|OUTPUT|BUNDLE]` reads only the built
`HAB0` envelope and its exact manifest. It applies the same regular-file and size
limits used by production worker startup, verifies the envelope and manifest
digest, validates the HTA document, and reports the runtime ABI, digest, and
embedded `HBX0` size without executing application code.

The Nginx module passes configured paths to the Rust runtime rather than
allocating files in C. Bundle and manifest limits are therefore checked before
allocation by one library-owned implementation.

## Production artifact projection

Development builds may emit `.hoplite/app.hal` as an inspectable projection.
Hoplite excludes the entire generated `.hoplite` tree from application source
registration, including projects whose source path is the project root.
Production builds remove `app.hal` and retain only alpha serving artifacts,
generated configuration, platform documents, and API descriptions.

The generic container image copies only `/app/.hoplite` from the builder stage.
Application `.hal` files, `project.edn`, the development `hoplite` CLI, Cargo,
Rust, and native build tools are absent from the final image. The production
image gate verifies this composition before exercising the real Nginx serving
path.

# Hoplite application bundle v1

Production workers boot one **Hoplite application bundle** rather than treating
compiled Hara modules and the route manifest as unrelated inputs.

## Format

The byte layout is deterministic:

```text
HAB1
sha256(payload)
runtime ABI (u32 little-endian)
sha256(exact apps.hta bytes)
embedded bytecode length (u32 little-endian)
embedded Hara HBB2 bytecode bundle
```

`HAB1` is the Hoplite-owned compatibility boundary. `HBB2` remains the
Hara-owned container for namespace-preserving bytecode modules.

The v1 decoder rejects:

- unsupported runtime ABI versions;
- an empty or oversized route manifest;
- an empty, oversized, or non-HBB2 bytecode payload;
- checksum, manifest-digest, and declared-length mismatches;
- truncation and trailing bytes.

The current limits are 8 MiB for `apps.hta` and 64 MiB for embedded HBB2
bytecode.

## Startup law

The worker validates the complete HAB1 envelope, then performs a full
bytecode-and-route preflight on an isolated thread. Hara's thread-local protocol
and multimethod state cannot leak from a rejected preflight. Only after that
succeeds does Hoplite construct a fresh staged runtime, load the same HBB2 bytes,
prepare the same route handlers, and atomically replace the pristine worker
runtime.

A bytecode failure, unknown handler, malformed manifest, checksum mismatch, or
manifest substitution leaves the serving runtime with neither partially loaded
namespaces nor partially prepared routes. Combined application bootstrap is
therefore accepted only on a fresh runtime.

## Compatibility

The bundle records the exact Hoplite runtime ABI. Changing the envelope layout,
manifest-binding law, or required runtime ABI requires a new HAB format and a
documented migration path.

The lower-level `hoplite_bootstrap_bytecode` and `hoplite_apps_prepare` symbols
remain available for embedding compatibility, but the generated Nginx
production configuration uses `hoplite_bootstrap_application_v1`.

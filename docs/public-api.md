# Hoplite public application and embedding surfaces

This is the human-readable compatibility reference for Hoplite alpha. The
machine-readable inventory is [`public-surfaces.json`](public-surfaces.json),
whose identity is `hoplite.public-surfaces/0-alpha`.

The inventory is deliberately broader than the public API. It classifies every
current `hoplite.*` HAL namespace, shipped Hoplite C header, compiled Rust
binary, supported CLI command, and portable document so that repository
presence cannot accidentally create a compatibility promise.

## Compatibility statuses

| Status | Meaning during alpha |
| --- | --- |
| `public` | Supported application, document, CLI, or embedding surface. An incompatible change requires a deliberate version decision, migration note, and focused conformance evidence. |
| `experimental` | Available for evaluation and allowed to change within the current alpha epoch. The changing pull request must update documentation and behavioural fixtures. |
| `migration-only` | Historical product or compatibility surface retained only while it is extracted or retired. New generic Hoplite code must not depend on it. |
| `internal` | Implementation detail with no source or binary compatibility promise. |

Package versions, portable-document identities, and native ABI generations are
independent version spaces; see [Versioning during alpha](versioning.md).

### Pre-1.0 deprecation law

A public HAL, CLI, or behavioural surface is announced as deprecated in one
published pre-1.0 package release and is not removed in that same release.
Removal requires a later package release and a migration note.

A public native ABI symbol is never repurposed with a new call shape. A changed
shape receives a new versioned symbol or ABI generation. The previous symbol
remains available for at least one published package release after its
replacement is documented, except where an uncontainable security or
memory-safety defect makes continued exposure unsafe.

An incompatible portable-document change receives a new explicit format
identity or alpha epoch. Readers reject unsupported identities rather than
treating them as aliases. A migration fixture records rejection or
transformation of the prior identity.

Experimental surfaces have no deprecation window. Migration-only surfaces may
be removed by the extraction change that records their destination and
preserves generic invariants at the correct interface boundary. Internal
surfaces may change without notice.

## HAL application surface

### `hoplite.core` — public

`hoplite.core` constructs immutable, tagged application values:

- `app` selects an application definition or handler;
- `response` constructs a logical response value;
- `stream` marks a producer as a backpressured response stream;
- `middleware`, `service`, `module`, `adapter`, and `store` construct composable
  definitions;
- `package-ref` selects an exact package export and local alias.

The selected project profile names a qualified application Var. Hoplite loads
application namespaces, evaluates that Var, validates routes, resolves handler
Vars, and prepares the worker-local dispatch table before publishing the serving
runtime. Failed application or route preflight must not partially mutate the
published runtime.

Routes are immutable data under `:resources`. Each operation carries a method,
path, handler Var, optional name and summary, and exactly one adapter:

- `:raw` exposes the borrowed native exchange and imperative response API;
- `:request` exposes the request view and accepts logical response maps;
- `:request+hta` materializes a portable HTA request before invocation.

Unknown adapters fail during application validation. Method/path matching and
handler preparation occur before request execution; production dispatch must not
parse or compile application source per request.

A request-body policy is application configuration. Request-body handles are
opaque request-scoped capabilities, never paths, descriptors, credentials, or
portable authority. `:request+hta` routes cannot be combined with the native
request-body policy because eager materialization would violate the bounded
streaming ownership model.

### `hoplite.raw` — public

Accessors expose method, URI, path, query string, remote address, and headers
from the borrowed exchange. `respond!`, `start!`, `write!`, and `finish!`
operate on that exchange.

The exchange is valid only for the active request invocation. Applications must
not retain it after completion, cancellation, disconnect, or worker shutdown.
The native host owns the exchange and backing memory.

### `hoplite.response-source` — experimental

`hoplite.response-source/0-alpha` is a closed descriptor containing exactly
`:protocol`, `:service`, `:source-handle`, `:offset`, and `:length`. Handles and
bounds are portable safe integers; handles and lengths are positive, offsets
are non-negative, and the half-open interval stays inside the safe range.

A numeric handle is not authority by itself. The host binds it to the exact
request/work that created it. Reads, cancellation, completion, disconnect, and
shutdown converge on exactly-once close. Stale, foreign-work, malformed, and
already-closed handles fail before a provider callback runs.

### Other HAL namespaces

`hoplite.host` is experimental convenience over generic host calls. Application
semantics must remain provider-replaceable.

`hoplite.dev` is development-only and is not part of source-free production
startup.

`hoplite.auth` and `hoplite.value` are migration-only historical
policy/provider helpers. `hoplite.value` is physically quarantined beneath
`migration/value`, excluded from ordinary resource registration and HAB0/HBX0
application construction, and available only with `legacy-value-contract` for
path-scoped compatibility evidence. Neither namespace is generic runtime API.

`hoplite.internal` is implementation-only. Applications importing it receive no
compatibility promise.

## Portable documents

### HAB0 application envelope — public alpha contract

The current application document is
`hoplite.application-bundle/0-alpha`, identified by `HAB0`. It binds runtime ABI
4, the SHA-256 digest of the exact route manifest, the complete Hara `HBX0`
namespace bundle, declared lengths, and a complete payload checksum. HBX0
contains ordered Hara `HBC0` modules.

Validation is bounded and precedes application preflight. Unsupported ABI,
invalid magic, non-HBX0 payload, truncation, trailing bytes, length drift,
manifest substitution, and checksum mismatch are errors. Superseded
stable-looking outer and inner markers are rejected rather than accepted as
aliases. See [Application bundle](application-bundle.md).

### Request body — public

`hoplite.request-body/3` is the native request-body boundary. The runtime owns a
transferred descriptor after top-level pointer preflight. Reads are bounded, a
declared length may be required, and completion or failure closes the
descriptor exactly once.

### Provider HTA — migration-only

`hoplite.provider-hta/1` belongs to the historical provider-product release
path. It remains documented only for extraction compatibility and must not
shape generic startup or the required release gate.

## Native embedding surfaces

### `hoplite_runtime.h` — public

The runtime ABI is usable without the `hoplite` process. An embedder creates one
opaque `hoplite_runtime_t`, performs bootstrap and handler preparation, invokes
requests/work, and finally frees the runtime.

Runtime state is worker-local. A runtime, handler, work, response, body handle,
or buffer must not cross Nginx workers or outlive its owning runtime. Unless an
API explicitly transfers ownership, input slices are borrowed only for the call.

`hoplite_runtime_new` and `hoplite_runtime_free` own runtime lifecycle.
`hoplite_bootstrap_application_v1` and
`hoplite_bootstrap_application_files_v1` validate HAB0 plus the exact manifest
and transactionally publish one complete staged runtime. The `_v1` suffix names
the C call shape; it is independent of the `/0-alpha` document epoch.

Prepared handler, app, work, call, response, and request-body identifiers are
opaque runtime-local handles. Closing an object consumes authority to use that
handle. Unknown, stale, cross-runtime, and already-closed handles are errors.

A `hoplite_buffer_t` returned by the runtime is released only with
`hoplite_buffer_free`, using the exact pointer and length received. Rust, Hara,
Nginx, and extension allocators are not interchangeable.

A `VmFiberState::Yielded` escaping without a supported coroutine driver is
reported as `fiber/yield-unsupported`. Promise-backed suspension is supported
through the work/event/call boundary.

### `hoplite_data_plane.h` — public

The data-plane header defines versioned request- and response-body callback
descriptors. Callback context is opaque. The Rust bridge never interprets it as
a path, URL, file descriptor, credential, or application value.

An accepting API may transfer exclusive lifecycle ownership. After transfer,
the caller must not reuse the descriptor or call its close callback. Callback
storage and context remain valid until runtime completion or close.

### `hoplite_host_provider.h` — experimental

The generic provider ABI registers immutable descriptors during trusted worker
startup. Registration and lookup are exact and case-sensitive. Descriptor
storage, callback pointers, and service bytes outlive the worker. Request data
cannot mutate the registry.

Provider calls carry request context, work, call, operation, standalone HTA
arguments, and completion callbacks. Cancellation and `release_work` release
retained state without leaking request-body or response-source ownership.

### Internal and migration-only native headers

`hoplite_host_registry.h` and `hoplite_hta.h` are worker implementation details.
Blob, value, store, and composed value/store provider headers are
migration-only. Their transport, corruption, cancellation, and cleanup
invariants must remain at generic host/data-plane boundaries when product
implementations are extracted.

## CLI surfaces

The public `hoplite` control CLI supports `repl`, `eval`, `run`, `doctor`,
`inspect`, `verify`, `package`, `serve`, and `version`. `auth` exists only in builds with the
migration-only `legacy-management` feature. Unknown commands and operational
failures exit non-zero and write an error prefixed with `hoplite:` to standard
error. Successful commands and help exit zero.

`hoplite inspect` validates only generated HAB0 and HTA bytes, reports route and
adapter counts plus generated-artifact digests, detects source inputs beneath the
output directory, and redacts filesystem paths unless `--show-paths` is supplied.
Its JSON identity is `hoplite.inspect/0-alpha`; see
[Diagnostics](diagnostics.md).

`hoplite doctor` diagnoses the executable runtime, Nginx, CA trust, project
profile and source declarations, package-lock/platform consistency, and any
generated HAB0 application. Its default pass does not execute source; `--deep`
explicitly performs compilation and application preflight. Paths remain redacted
unless `--show-paths` is supplied. Its JSON identity is
`hoplite.doctor/0-alpha`.

`hoplite-server` consumes generated `.hoplite` output and supports production
serving, a bounded worker-count override, help, and version reporting. Failures
exit non-zero with a `hoplite-server:` prefix. The final production image must
not require application source, Cargo, a Hara source compiler, or the
development CLI.

`package` remains experimental while historical provider artifact operations
are separated from generic application packaging. The object/provider lock
generator binaries are migration-only source-build tools. The
bytecode-loading program is internal measurement evidence, not a supported user
command.

## Conformance and release review

The required Rust workspace test reads `public-surfaces.json` and enforces:

- the exact `hoplite.public-surfaces/0-alpha` identity;
- allowed statuses, unique identities, and referenced-file existence;
- complete classification of `core/lib/src/hoplite/*.hal`;
- complete classification of all shipped Hoplite C headers;
- complete classification of `core/src/main.rs` and `core/src/bin/*.rs`;
- explicit program ownership for CLI commands;
- explicit version identities for portable documents.

A release review additionally confirms that public behaviour has focused tests,
migration-only code has not entered the default dependency tree or production
image, errors expose stable classes without secrets or native pointers, and
incompatible changes follow the version/deprecation law above.

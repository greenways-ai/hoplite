# Hoplite public application and embedding surfaces

This document is the human-readable compatibility reference for Hoplite alpha.
The machine-readable inventory is [`public-surfaces.json`](public-surfaces.json),
whose contract identity is `hoplite.public-surfaces/0-alpha`.

The inventory is deliberately broader than the public API. It classifies every
current `hoplite.*` HAL namespace and every shipped Hoplite C header as one of
four statuses so that a source file cannot become public merely by remaining in
the repository.

## Compatibility statuses

| Status | Meaning during alpha |
| --- | --- |
| `public` | Supported application, document, CLI, or embedding surface. An incompatible change requires a deliberate version decision, a migration note, and focused conformance evidence. |
| `experimental` | Available for evaluation but still allowed to change within the current alpha epoch. The changing pull request must update its documentation and behavioural fixtures. |
| `migration-only` | Historical product or compatibility surface retained only while it is extracted or retired. New generic Hoplite code must not depend on it. |
| `internal` | Implementation detail with no source or binary compatibility promise. |

A public surface is not promised to be stable merely because its package is
`0.1.x`. Package versions, portable-document identities, and native ABI
versions are independent spaces; see [Versioning during alpha](versioning.md).

### Pre-1.0 deprecation law

For a public HAL, CLI, or behavioural surface, Hoplite announces deprecation in
one published pre-1.0 release and does not remove the surface in that same
release. Removal requires a later package release and a migration note.

For a public native ABI, an existing versioned symbol is never repurposed with a
new call shape. A changed call shape receives a new versioned symbol or ABI
generation. The previous symbol remains available for at least one published
package release after the replacement is documented, except for a security or
memory-safety defect that cannot be contained safely.

For a portable document, an incompatible change receives a new explicit format
identity or alpha epoch. Readers reject unsupported identities; they do not
silently treat an older or newer marker as an alias. A migration fixture records
how the prior identity is rejected or transformed.

Experimental surfaces have no deprecation window. Migration-only surfaces may
be removed by the extraction change that records their destination and preserves
any generic invariant at the correct interface boundary. Internal surfaces may
change without notice.

## HAL application surface

### `hoplite.core` — public

`hoplite.core` constructs immutable application values. The supported
constructors are ordinary Hara functions and return closed tagged maps:

- `app` selects one application definition or handler;
- `response` constructs a logical response value;
- `stream` marks a producer as a backpressured stream;
- `middleware`, `service`, `module`, `adapter`, and `store` construct composable
  definitions;
- `package-ref` selects one exact package export and binds it to a local alias.

The selected project profile must name a qualified application Var. Hoplite
loads application namespaces, evaluates the selected Var, validates all route
definitions, resolves every handler Var, and prepares the worker-local dispatch
table before publishing the serving runtime. A failed application or route
preflight must not partially mutate the published worker runtime.

Routes are immutable data under `:resources`. Each operation has a method, path,
handler Var, optional name and summary, and one route adapter. The default
adapter is `:request`; the supported adapter identities are exactly:

- `:raw` — exposes the borrowed native exchange and imperative raw response
  functions;
- `:request` — exposes the request view and accepts ordinary logical response
  maps;
- `:request+hta` — materializes a portable HTA request before invoking the
  handler.

Unknown adapters fail during application validation. Method/path matching and
handler preparation occur before request execution; production dispatch must not
parse or compile application source per request.

A request-body policy is application configuration. Request-body handles remain
opaque, request-scoped capabilities. They are not paths, descriptors,
credentials, or portable authority. `:request+hta` routes cannot be combined
with the native request-body policy because materializing the request would
violate the bounded streaming ownership model.

### `hoplite.raw` — public

`hoplite.raw` exposes the lowest-level route adapter. Accessors return method,
URI, path, query string, remote address, and headers from the borrowed exchange.
`respond!`, `start!`, `write!`, and `finish!` operate on that exchange.

The exchange is valid only for the active request invocation. Applications must
not retain it after completion, cancellation, disconnect, or worker shutdown.
The native host owns the exchange and its backing memory. Raw response
operations must either complete the response or leave cleanup to the host; they
must not transfer ownership into application-global state.

### `hoplite.response-source` — experimental

`hoplite.response-source/0-alpha` is a closed portable descriptor containing
exactly `:protocol`, `:service`, `:source-handle`, `:offset`, and `:length`.
Handles and bounds must be safe integers; handles and lengths are positive,
offsets are non-negative, and the half-open interval must remain inside the
portable safe-integer range.

The numeric handle is not authority by itself. The host binds it to the exact
request/work that created it. Reads, cancellation, completion, disconnect, and
shutdown all converge on exactly-once close. A stale, foreign-work, malformed,
or already-closed handle fails before a provider callback runs.

### Experimental, migration-only, and internal HAL namespaces

`hoplite.host` is the experimental convenience layer over generic host calls.
Its logical behaviour must remain provider-replaceable: application semantics
cannot depend on one concrete extension implementation.

`hoplite.dev` is development-only. It is not linked into the source-free
production startup contract.

`hoplite.auth` and `hoplite.value` are migration-only historical application
policy/provider helpers. They are extraction input, not generic runtime API.

`hoplite.internal` is reserved for build and runtime implementation values. An
application that imports it accepts that there is no compatibility promise.

## Portable documents

### HAB0 application envelope — public alpha contract

The current application document is
`hoplite.application-bundle/0-alpha`, identified by outer magic `HAB0`. It binds
runtime ABI 4, the SHA-256 digest of the exact route manifest, the complete Hara
`HBX0` namespace bundle, declared lengths, and a complete payload checksum.
HBX0 in turn contains ordered Hara `HBC0` modules.

Validation is bounded and precedes application preflight. Unsupported ABI,
invalid magic, non-HBX0 payload, truncation, trailing bytes, length drift,
manifest substitution, and checksum mismatch are errors. The superseded
stable-looking HAB1/HBB2 markers are rejected rather than accepted as aliases.
See [Application bundle](application-bundle.md).

### Request body — public native document generation

`hoplite.request-body/3` represents the current native request-body boundary.
The runtime owns a transferred body descriptor after top-level pointer
preflight. Reads are bounded, a declared length can be required, and completion
or failure closes the descriptor exactly once.

### Provider HTA — migration-only

`hoplite.provider-hta/1` belongs to the historical provider-product release
path. It remains documented only for extraction compatibility and must not shape
the generic application startup or release gate.

## Native embedding surfaces

### `hoplite_runtime.h` — public

The runtime ABI is usable without the `hoplite` CLI process. An embedder creates
one opaque `hoplite_runtime_t`, performs bootstrap and handler preparation, then
invokes requests/work and finally frees the runtime.

Runtime state is worker-local. A runtime, handler, work, response, body handle,
or buffer must not be shared between Nginx workers or used after its owning
runtime is freed. Unless an API explicitly documents a transfer, input slices
are borrowed only for the duration of the call.

`hoplite_runtime_new` and `hoplite_runtime_free` own the runtime lifecycle.
`hoplite_bootstrap_application_v1` and
`hoplite_bootstrap_application_files_v1` validate HAB0 plus the exact manifest
and publish one complete staged runtime transactionally. The `_v1` suffix names
the C call shape; it is independent of the `/0-alpha` document epoch.

Prepared handler, app, work, call, response, and request-body identifiers are
opaque runtime-local handles. Closing an object consumes the caller's authority
to use that handle. Unknown, stale, cross-runtime, or already-closed handles are
errors.

`hoplite_buffer_t` returned by the runtime is released only with
`hoplite_buffer_free`. The caller must pass the exact data pointer and length it
received. Rust, Hara, Nginx, and extension allocators are not interchangeable.

A `VmFiberState::Yielded` that escapes without a supported coroutine driver is
reported as `fiber/yield-unsupported`. Promise-backed suspension is supported
through the work/event/call boundary.

### `hoplite_data_plane.h` — public

The data-plane header defines the versioned request- and response-body callback
descriptors. Native callback context is opaque. The Rust bridge never interprets
it as a path, URL, file descriptor, credential, or application value.

An API that accepts a descriptor may transfer exclusive lifecycle ownership.
After transfer, the caller must not reuse the descriptor or call its close
callback. Callback storage and context must remain valid until the owning
runtime reports completion or performs close.

### `hoplite_host_provider.h` — experimental

The generic host-provider ABI registers immutable provider descriptors during
trusted worker startup. Registration and lookup are exact and case-sensitive.
Descriptor storage, callback pointers, and service bytes outlive the worker.
Request data cannot mutate the registry.

Provider calls carry request context, work, call, operation, standalone HTA
arguments, and completion callbacks. Providers may complete immediately or
retain the request according to the declared lifecycle. Cancellation and
`release_work` must release retained state without leaking request-body or
response-source ownership.

### Internal and migration-only native headers

`hoplite_host_registry.h` and `hoplite_hta.h` are worker implementation details.
Blob, value, store, and composed value/store provider headers are
migration-only. Their generic transport, corruption, cancellation, and cleanup
invariants must be retained through the generic host/data-plane boundaries when
the product implementations are extracted.

## CLI surfaces

The public `hoplite` control CLI supports `repl`, `eval`, `run`, `verify`,
`package`, `serve`, and `version`. `auth` is available only when the
migration-only `legacy-management` feature is built. Unknown commands and
validation/operational failures exit non-zero and write an error prefixed with
`hoplite:` to standard error. Successful commands and help exit zero.

`hoplite-server` is the production-serving program. It consumes generated
`.hoplite` output and must not require application source, Cargo, a Hara source
compiler, or the development CLI in the final production image.

`package` remains experimental because historical provider artifact operations
are still being separated from generic application packaging. Its presence does
not make provider release locks part of the supported runtime API.

## Conformance and release review

The required Rust workspace test reads `public-surfaces.json` and enforces:

- the exact `hoplite.public-surfaces/0-alpha` identity;
- allowed compatibility statuses and unique names;
- existence of every referenced source/document;
- complete classification of all `core/lib/src/hoplite/*.hal` namespaces;
- complete classification of all shipped Hoplite C headers;
- explicit version identities for portable documents.

A release review must additionally confirm that public behaviour is covered by
focused tests, migration-only code has not entered the default dependency tree
or production image, errors expose stable classes without secrets or native
pointers, and any incompatible change follows the version/deprecation law above.

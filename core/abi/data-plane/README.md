# Hoplite data-plane ABI

This crate defines the application-neutral transport contracts required by
Tahto and other large-object Hoplite applications.

It exists because the ordinary Hoplite/Hara route boundary is intentionally
value-oriented: request metadata becomes a Hara request value and ordinary
response bodies become strings or byte values. Multi-megabyte and
multi-gigabyte objects must not cross that boundary as ordinary Hara values.

## Contracts

### Bounded request streaming

`BodyLimits`, `BodyAccount`, `RequestBody`, and `BoundedBody` enforce:

- a maximum request body size;
- a maximum chunk size;
- optional mandatory `Content-Length` declaration;
- preflight rejection when the declared size is too large;
- cumulative accounting while bytes arrive;
- rejection when observed bytes exceed the declaration; and
- exact declaration verification when a body finishes.

A runtime adapter exposes only an opaque server-owned body handle to Hara. Hara
may authorize the operation and choose an application-owned storage action, but
the object bytes remain in the native data plane.

### Streaming and range responses

`ResponseBody`, `ByteRange`, `ResponsePlan`, and `StreamResponse` support:

- seekable native response sources;
- full responses with `Accept-Ranges: bytes`;
- one exact, open-ended, or suffix byte range;
- `206 Partial Content` planning;
- bounded chunk reads; and
- early-EOF and source-contract checks.

Multiple ranges are deliberately rejected in the first ABI. A response source
is a server-owned resource handle, not a request-selected filesystem path or
upstream URL.

### Signed application-device authentication

`SignedDeviceRequest` fixes the exact production signing fields:

```text
hoplite-signed-device/0-alpha
method
target
authority
content digest
operation
application
namespace
collection
timestamp
nonce
idempotency key
key id
```

The encoded signature is carried beside those fields and is never part of the
signing input. Version 2 is necessary because the earlier
`hoplite-signed-device/0-alpha` profile did not bind the application operation,
coordinates, or idempotency key. Production providers must not accept version
1 as an alias for version 2.

Targets are origin-relative, authorities cannot contain userinfo or paths,
content digests are lower-case SHA-256 identifiers, and line-delimited fields
reject whitespace or delimiter ambiguity before a provider is invoked.

`ApplicationRequestExpectation` contains trusted facts derived from the actual
HTTP exchange and selected route. `authenticate_application_request` compares
every signed transport and application field against those facts before
calling a provider. It then projects only a closed application identity and
returns `hoplite-verified-application-request/0-alpha` evidence.

`SignedDeviceProvider` is application-neutral. An installed provider owns key
lookup, clock policy, signature verification and revocation checks. Durable
nonce/idempotency admission remains a separate atomic boundary: a valid
signature alone never means that a nonce has been consumed.

### Safe application identity projection

`ApplicationIdentity::project` accepts only the `application` realm and
requires:

```text
application/id
application/version
application/publisher
application/lock-digest
application/namespace
application/collection
application/operations
```

Only an allowlisted claim set reaches the Hara application. Provider identity,
bearer tokens, session identifiers, management claims, administrator
credentials and raw device keys are removed. The adapter also checks the
projected application, namespace, collection and allowed operation against the
signed request.

### Opaque resource handles

`ResourceHandle` is a non-zero server-assigned integer. It intentionally carries
no path, URL, upstream, file descriptor, or executable instruction. Runtime
adapters resolve it against request-scoped or response-scoped native
registries.

## Security laws

A conforming adapter must never:

- let a request choose a filesystem path, provider or public key;
- let a request choose a proxy upstream;
- materialize a large request or response body as an ordinary Hara value;
- verify a signature before matching signed fields to the actual request;
- project a management principal into the application realm;
- expose provider identity, signatures, access or refresh tokens to handlers;
- infer administrator authority from device enrolment;
- treat signature verification as durable replay admission; or
- treat an opaque resource handle as portable outside its server scope.

## Integration sequence

1. `hoplite-signed-device-ed25519` verifies the exact version-2 signing bytes
   using trusted key and clock configuration.
2. The runtime supplies actual request/route expectations and projects the
   closed verified evidence into the handler request.
3. A durable admission provider atomically consumes nonce/idempotency evidence
   across worker and process restart.
4. The Tahto fixture uses the resulting generic boundary without moving its
   authorization or state-transition semantics into Hoplite.

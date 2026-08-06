# Hoplite data-plane ABI

This crate defines the application-neutral transport contracts required by Tahto and other large-object Hoplite applications.

It exists because the ordinary Hoplite/Hara route boundary is intentionally value-oriented: request metadata becomes a Hara request value and ordinary response bodies become strings or byte values. Multi-megabyte and multi-gigabyte objects must not cross that boundary as ordinary Hara values.

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

A runtime adapter exposes only an opaque server-owned body handle to Hara. Hara may authorize the operation and choose an application-owned storage action, but the object bytes remain in the native data plane.

### Streaming and range responses

`ResponseBody`, `ByteRange`, `ResponsePlan`, and `StreamResponse` support:

- seekable native response sources;
- full responses with `Accept-Ranges: bytes`;
- one exact, open-ended, or suffix byte range;
- `206 Partial Content` planning;
- bounded chunk reads; and
- early-EOF and source-contract checks.

Multiple ranges are deliberately rejected in the first ABI. A response source is a server-owned resource handle, not a request-selected filesystem path or upstream URL.

### Signed-device authentication

`SignedDeviceRequest` fixes the canonical request fields used by a provider:

```text
method
target
authority
content digest
timestamp
nonce
key id
signature
```

The signing input is domain-separated by `hoplite-signed-device/1`. Targets must be origin-relative, authorities cannot contain userinfo or paths, and content digests use lower-case SHA-256 identifiers.

`SignedDeviceProvider` is a provider contract, not a built-in Tahto authenticator. Hoplite owns the transport and realm projection; applications or installed provider packages own key lookup, nonce persistence, clock policy, signature verification, and revocation checks.

### Safe application identity projection

`ApplicationIdentity::project` accepts only the `application` realm and requires:

```text
application/id
application/version
application/publisher
application/lock-digest
```

Only an allowlisted claim set reaches the Hara application. Bearer tokens, session identifiers, management claims, administrator credentials, raw device keys, and provider-private claims are not projected.

### Opaque resource handles

`ResourceHandle` is a non-zero server-assigned integer. It intentionally carries no path, URL, upstream, file descriptor, or executable instruction. Runtime adapters resolve it against request-scoped or response-scoped native registries.

## Security laws

A conforming adapter must never:

- let a request choose a filesystem path;
- let a request choose a proxy upstream;
- materialize a large request or response body as an ordinary Hara value;
- project a management principal into the application realm;
- expose access or refresh tokens to application handlers;
- infer administrator authority from device enrolment; or
- treat an opaque resource handle as portable outside its server scope.

## Integration sequence

This PR introduces the compiled contract and conformance tests as the first reviewable HOPLITE-1 slice. Runtime and Nginx wiring should implement these traits and handles without changing the contract. Tahto then binds its object-vault operations to the resulting native adapter.

# Exact-span HTA for native providers

Generic native providers receive one standalone `HTA1` frame containing the Hara host-call argument vector. They must be able to inspect the closed request envelope without reconstructing opaque nested application values.

`hoplite-provider-hta` is the shared dependency-free reader for that boundary. It:

- validates one complete canonical frame;
- applies strict frame, depth, collection, text and byte-span limits;
- rejects native handles by default;
- rejects malformed UTF-8, duplicate keys and non-canonical map/set order;
- records the exact encoded span of every nested value;
- supports exact string/keyword map lookup and vector indexing; and
- can copy one nested value into a standalone `HTA1` frame.

This lets `hoplite.store` persist the exact `:value` and `:receipt` spans and lets `hoplite.blob` validate closed requests without introducing application-specific decoders.

The parser does not interpret Tahto records, storage semantics, authorization, paths, drivers or credentials. Provider-specific adapters layer their own exact closed-field validation over this common reader.

## Authority boundary

Parsing a frame does not grant access to a native resource. Work-scoped request and response handles remain resolvable only through the Hoplite provider APIs using the exact owning request and work scope.

## Canonical ordering

HTA maps and sets use the canonical bare encoded ordering produced by Hara's HTA encoder. The provider reader requires strictly increasing entries, which rejects both reordered and duplicate keys or set members.

`OBJECT` values retain insertion order but require unique UTF-8 string keys. They are supported for transport completeness; provider request profiles should prefer canonical maps.

# Canonical value digests

`hoplite.host/canonical-value-digest` hashes the exact canonical standalone
HTA0 encoding of one portable Hara value and returns a lowercase
`sha256:<hex>` digest.

This operation lets an application prepare a generic `hoplite.store`
initialization or compare-and-swap request without substituting JSON bytes for
the provider's canonical value encoding. It is application-neutral: the host
does not inspect Tahto records or grant storage authority.

The encoded frame is limited to 8 MiB. Unsupported values, encoding failures,
oversized frames and incorrect argument counts fail closed.

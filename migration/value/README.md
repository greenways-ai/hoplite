# Legacy `hoplite.value` compatibility material

This directory contains the historical bounded canonical-value HAL contract.
It is migration input, not part of the default Hoplite runtime, application
bundle, production image, public extension model, or required merge gate.

The exact namespace remains available only with the non-default Cargo feature
`legacy-value-contract`. Its focused conformance runs in the path-scoped
Compatibility workflow. Request data cannot enable this feature or select the
contract at runtime.

## Compatibility window

The feature remains available for one documented pre-1.0 migration window while
consumers move to the owning value-provider package. New Hoplite applications
must not import `hoplite.value`. The eventual extraction or retirement change
must preserve any generic boundedness, canonical decoding, digest, corruption,
and failure-shape invariant at the appropriate provider or data-plane boundary.

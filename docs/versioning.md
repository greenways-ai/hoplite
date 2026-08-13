# Versioning during alpha

Hara and Hoplite are in a pre-release development period. Their version labels
belong to distinct compatibility spaces and must not be interpreted as one
global maturity number.

## Distribution versions

Cargo crates, Homebrew formulae, container tags, and project packages use
ordinary pre-1.0 semantic versions such as `0.1.x`. Those numbers allow package
managers to order artifacts. They do not declare the language, HTTP runtime, or
portable contracts stable.

A distribution version therefore does not determine the version embedded in a
wire document, bytecode artifact, native structure, or exported symbol.

## Portable formats and protocols

Evolving Hara- and Hoplite-owned portable documents use an explicit alpha epoch:

| Surface | Current identity |
| --- | --- |
| Hara bytecode artifact | `HBC0` |
| Hara bytecode bundle | `HBX0` |
| Hoplite application bundle | `hoplite.application-bundle/0-alpha` / `HAB0` |
| Hoplite doctor report | `hoplite.doctor/0-alpha` |
| Hoplite inspection report | `hoplite.inspect/0-alpha` |
| Other evolving owned contracts | `<contract>/0-alpha` |

An incompatible alpha change may replace an epoch or marker, but code, fixtures,
documentation, and migration notes must change together. Stable-looking
portable major versions are not introduced until that individual surface is
deliberately frozen.

Independently mature or migration-only formats are not reset merely because a
neighbouring surface is in alpha. Each contract names and enforces its own
compatibility law.

## Native ABI and symbol revisions

Numeric runtime ABI values and suffixes such as `_v1`, `_v2`, and `_v3` identify
native structure or function shapes. For example, Hoplite runtime ABI `4` remains
the current embedding compatibility value. These identifiers are independent of
portable document maturity and change only when their binary shape or calling
contract changes.

## Repository rule

A change conforms to the alpha direction when:

1. evolving portable contracts use their declared alpha identity;
2. package-manager versions remain valid pre-1.0 semantic versions;
3. native ABI generations change only with their binary contract;
4. tests reject obsolete format markers rather than silently accepting them;
5. user-facing documentation names the same identities enforced by code; and
6. an incompatible alpha change includes an explicit migration note.

`packaging/scripts/verify-alpha-versioning.sh` protects these invariants in CI.

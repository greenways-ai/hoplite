# Hoplite alpha release checklist

A Hoplite alpha release is cut only from one reviewed tag commit. The tag binds
one Hoplite source revision, one reviewed Hara source revision, one reviewed
Hara Native revision, one committed Cargo lock, one public-surface inventory,
one core-boundary inventory, and one set of successful permanent CI checks.

The release workflow enforces the mechanical parts of this checklist in its
`prepare` job before building or publishing any artifact.

## 1. Immutable release inputs

- The release tag starts with `v` and its version exactly matches
  `core/Cargo.toml`.
- The workflow resolves the full Hoplite commit SHA from the tag.
- `packaging/hara-revision` and `packaging/hara-native-revision` each contain
  one full 40-character reviewed commit SHA; both checkouts are detached at
  those exact revisions.
- `core/Cargo.lock` resolves without mutation through `cargo metadata --locked`.
- The repository alpha-version policy passes at the tagged source revision.

A manually dispatched rebuild may supply a Hara revision only to reproduce a
legacy tag whose source did not yet contain `packaging/hara-revision`. It must
still be a complete existing commit SHA.

## 2. Required evidence on the tag commit

The exact Hoplite commit referenced by the tag must already have successful
GitHub check runs named:

- `library`;
- `integration`;
- `docs`.

The release workflow queries the check runs for the immutable commit SHA. A
missing, queued, cancelled, neutral, skipped, timed-out, stale, or failed check
blocks publication. A similarly named check on another commit does not count.

These checks jointly prove:

- alpha contract and reviewed Hara pin policy;
- locked Rust workspace tests;
- complete public-surface and core-boundary inventories;
- the exact default binary and dependency boundary;
- native C/header/provider/response-source conformance;
- source-free final-image composition;
- real Nginx request handling and multi-module dispatch;
- documentation buildability.

When executable embedding conformance is part of `CI / library`, the same gate
also proves the public Rust/C runtime lifecycle and exact native symbol set.

## 3. Contract inventories

Before artifact construction, both machine-readable registries must parse as
JSON and carry their current alpha identities:

- `docs/public-surfaces.json` — `hoplite.public-surfaces/0-alpha`;
- `docs/core-boundary.json` — `hoplite.core-boundary/0-alpha`.

Every public or experimental surface changed since the previous tag needs:

- updated compatibility documentation;
- focused behavioural evidence;
- an explicit version decision when the change is incompatible;
- a migration note when an existing public surface is deprecated or removed.

The 0.2.0 retirement decision requires provider and migration product
inventories to remain empty. Historical database, authentication, storage, and
provider products must not re-enter the default executable set, dependency
boundary, production image, or public-surface promise.

## 4. Release artifacts

One successful release workflow produces all artifacts from the same prepared
revision triple:

- deterministic HARP package plus inspection and SHA-256 evidence;
- source-free production container;
- standalone macOS binaries for Apple Silicon and Intel;
- standalone Linux binaries for x86-64 and ARM64;
- GitHub release assets;
- a syntactically validated Homebrew formula;
- an optional central tap update when the repository secret is configured.

Each platform binary must execute a real Hara evaluation and report its version
before upload. The release workflow must not rebuild an artifact from a moving
branch or an unreviewed Hara checkout.

## 5. Human release review

Automation cannot decide release intent. The reviewer confirms:

- release notes identify user-visible application, runtime, CLI, ABI, document,
  diagnostic, and performance changes;
- known limitations and migration requirements are explicit;
- no credentials, native pointers, build paths, provider internals, or private
  request data appear in public diagnostics or evidence;
- benchmark claims point to retained measurement data and environment metadata;
- the starter application and primary README describe the current alpha command
  and artifact names;
- the tag is intentional and should be published.

## 6. Publication and post-release verification

The publish job runs only after every required artifact job succeeds. It creates
or updates the GitHub release for the existing verified tag, uploads all assets,
then renders the Homebrew formula from the immutable release metadata.

After publication, verify that:

- all expected assets and checksums are present;
- the container tag resolves and runs `hoplite-server version`;
- both `hoplite` and `hoplite-server` binaries report the released version;
- the Homebrew formula points to the same Hoplite/Hara source/Hara Native
  revisions and Nginx
  source checksum;
- documentation references the released alpha contract epoch.

A failed post-release verification is handled by correcting automation and
rebuilding the same tag only when the artifacts are reproducible from the exact
recorded inputs. Source changes require a new version and tag.

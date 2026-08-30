# Hoplite Homebrew release integration

The canonical public tap is `greenways-ai/homebrew-tap`. Users install Hoplite on macOS or Linux with the fully qualified formula name:

```shell
brew install greenways-ai/tap/hoplite
hoplite version
```

The formula builds from immutable Hoplite, Hara source, and Hara Native host
revisions plus the checksummed nginx source. This keeps one formula valid across
Apple Silicon, Intel macOS, x86-64 Linux and ARM64 Linux while release binaries
remain available as convenience artifacts.

## Release workflow

For every `v*` tag, `.github/workflows/release.yml`:

1. verifies that the tag matches `Cargo.toml`;
2. resolves the Hara source and Hara Native commits pinned in
   `packaging/hara-revision` and `packaging/hara-native-revision` and reuses
   them in every job;
3. builds the deterministic HARP package and container image;
4. builds and smoke-tests standalone binaries for both macOS and Linux architectures;
5. creates or updates the GitHub release idempotently;
6. renders a source formula pinned to the exact Hoplite, Hara source, and Hara
   Native commits;
7. updates `greenways-ai/homebrew-tap` after every release job succeeds.

Update both revision files deliberately whenever Hoplite moves to a compatible
Hara release. Each must contain one complete 40-character commit SHA. Hara
source is a build-side input used to compile application HBX0; Hara Native is
the host linked into Hoplite. The released `hoplite-server` has neither source
checkout in its final image.

The workflow can also be run manually against an existing tag. This is useful for rebuilding an older tag after release automation changes without moving or recreating that tag. Manual rebuilds use the release automation from the branch where the workflow is dispatched, while all application source remains pinned to the requested tag. When a legacy tag needs a different Hara source than its matching tag, provide the optional `hara_revision` input with the complete compatible Hara commit SHA. For example, rebuilding Hoplite `v0.1.0` requires `ba52a6bfce31aeff7359d0e60d7f8d1538204694`.

## Repository setup

1. Keep `greenways-ai/homebrew-tap` public so unauthenticated Homebrew clients can clone it.
2. Create a fine-grained GitHub token with contents write access to that repository.
3. Add it to the Hoplite repository as the `HOMEBREW_TAP_TOKEN` Actions secret.
4. Ensure the token can push to `main`, or adapt the final workflow step to the tap's protected-branch policy.

Without the secret, release assets and the rendered formula artifact are still published; only the central-tap update is skipped.

## Local formula rendering

Use sibling Hoplite, Hara, and Hara Native checkouts, then run:

```shell
cd hoplite/packaging/homebrew
make check
```

The Makefile derives the current versions, licence, nginx checksum and Git revisions and writes `Formula/hoplite.rb`. Override any value explicitly when reproducing a historical release:

```shell
make check \
  VERSION=0.1.0 \
  HOPLITE_REVISION=eaa09a2e3a54edce8a7d68d1cb887bb700a24afe \
  HARA_REVISION=ba52a6bfce31aeff7359d0e60d7f8d1538204694 \
  HARA_NATIVE_REVISION=9a248effc9c61b08a716560bb9f6d676afdbebfa \
  LICENSE=EPL-2.0
```

Copy the generated formula into a checkout of `greenways-ai/homebrew-tap`, then run that repository's macOS and Linux test workflow before merging it.

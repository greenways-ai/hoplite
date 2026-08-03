# Hoplite Homebrew release integration

The canonical public tap is `greenways-ai/homebrew-tap`. Users install Hoplite on macOS or Linux with the fully qualified formula name:

```shell
brew install greenways-ai/tap/hoplite
hoplite version
```

The formula builds from immutable Hoplite and Hara revisions and the checksummed nginx source. This keeps one formula valid across Apple Silicon, Intel macOS, x86-64 Linux and ARM64 Linux while release binaries remain available as convenience artifacts.

## Release workflow

For every `v*` tag, `.github/workflows/release.yml`:

1. verifies that the tag matches `Cargo.toml`;
2. resolves the Hara commit pinned in `packaging/hara-revision` and reuses it in every job;
3. builds the deterministic HARP package and container image;
4. builds and smoke-tests standalone binaries for both macOS and Linux architectures;
5. creates or updates the GitHub release idempotently;
6. renders a source formula pinned to the exact Hoplite and Hara commits;
7. updates `greenways-ai/homebrew-tap` after every release job succeeds.

Update `packaging/hara-revision` deliberately whenever Hoplite moves to a new Hara revision. The file must contain one complete 40-character commit SHA. For historical Hoplite tags that predate the file, the workflow first looks for a Hara tag with the same name and only falls back to the fetched Hara head with an explicit warning.

The workflow can also be run manually against an existing tag. This is useful for rebuilding an older tag after release automation changes without moving or recreating that tag. Manual rebuilds use the release automation from the branch where the workflow is dispatched, while all application source remains pinned to the requested tag and resolved Hara commit.

## Repository setup

1. Keep `greenways-ai/homebrew-tap` public so unauthenticated Homebrew clients can clone it.
2. Create a fine-grained GitHub token with contents write access to that repository.
3. Add it to the Hoplite repository as the `HOMEBREW_TAP_TOKEN` Actions secret.
4. Ensure the token can push to `main`, or adapt the final workflow step to the tap's protected-branch policy.

Without the secret, release assets and the rendered formula artifact are still published; only the central-tap update is skipped.

## Local formula rendering

Use sibling Hoplite and Hara checkouts, then run:

```shell
cd hoplite/packaging/homebrew
make check
```

The Makefile derives the current versions, licence, nginx checksum and Git revisions and writes `Formula/hoplite.rb`. Override any value explicitly when reproducing a historical release:

```shell
make check \
  VERSION=0.1.0 \
  HOPLITE_REVISION=eaa09a2e3a54edce8a7d68d1cb887bb700a24afe \
  HARA_REVISION=5ae3449e461274323318ceb33131111c53210835 \
  LICENSE=EPL-2.0
```

Copy the generated formula into a checkout of `greenways-ai/homebrew-tap`, then run that repository's macOS and Linux test workflow before merging it.

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

`.github/workflows/release-preflight.yml` validates a reviewed pull request
from `main` to the protected `release` branch. It calls the normal CI workflow,
checks the canonical release version and immutable source inputs, builds and
inspects HARP, checks the container build, and renders this formula without
publishing anything.

`.github/workflows/release.yml` is a manual promotion workflow that only
accepts the current `release` branch head. It creates or resumes a matching
draft release, builds the deterministic HARP package and platform binaries,
publishes and verifies the multi-platform container digest, renders the formula
from the exact Hoplite/Hara/Hara Native revisions, then finalizes and reads back
the GitHub release with `SHA256SUMS` and a release manifest. The central tap is
updated only after that verification succeeds.

Update both revision files deliberately whenever Hoplite moves to a compatible
Hara release. Each must contain one complete 40-character commit SHA. Hara
source is a build-side input used to compile application HBX0; Hara Native is
the host linked into Hoplite. The released `hoplite-server` has neither source
checkout in its final image.

If publication fails after the draft intent, correct the delivery problem and
rerun the same `release` commit. Existing tags and images are accepted only for
that exact recovery path. A source or version change requires a new version on
`main` and a fresh promotion.

## Repository setup

1. Create `release` from the intended `main` head and require pull requests plus
   the **Release preflight / release ready** check on that branch. Allow merge
   commits for the `main → release` promotion.
2. Keep `greenways-ai/homebrew-tap` public so unauthenticated Homebrew clients can clone it.
3. Create a fine-grained GitHub token with contents write access to the tap and
   ensure it can push to the tap’s `main`, or adapt the workflow to the tap’s
   protected-branch policy.
4. Create a `packages` environment restricted to Hoplite’s `release` branch,
   and add the token there as `HOMEBREW_TAP_TOKEN`.

Without the environment secret, release assets and the rendered formula are
still published; only the central-tap update is skipped.

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

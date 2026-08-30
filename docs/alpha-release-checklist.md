# Hoplite alpha release checklist

A Hoplite alpha release is promoted from one reviewed `main` commit to the
protected `release` branch. The promotion binds one Hoplite revision, one Hara
source revision, one Hara Native revision, one locked dependency graph, and the
current public-surface/core-boundary inventories.

## 1. Promote a reviewed release candidate

1. Make the release change on `main`, including the canonical version in
   `release/version.json`. It must exactly equal `core/Cargo.toml`.
2. Open a same-repository pull request from `main` to `release`; merge it with a
   merge commit. The release preflight runs full CI, verifies immutable inputs,
   builds and inspects the HARP package, checks the production container, and
   renders the Homebrew formula without publishing anything.
3. After the `release ready` check succeeds, dispatch **Release promotion** on
   the current `release` branch. It rejects every other ref, retests that exact
   commit, creates `v<version>` plus a draft release intent, and serializes
   promotion runs for the branch.

The `release` branch initially points at the intended `main` commit. Every
later promotion is an ordinary merge of `main` into `release`; the workflow
accepts that recurring topology rather than requiring `release` to remain an
ancestor of `main`.

## 2. Immutable release contract

- `release/version.json` uses `hoplite/release-version-v1` and contains valid
  SemVer matching the Hoplite Cargo package version.
- `packaging/hara-revision` and `packaging/hara-native-revision` contain full
  reviewed commit SHAs; every release job checks out those exact revisions.
- `core/Cargo.lock` resolves with `--locked`; alpha versioning and the public
  surface/core-boundary inventories pass their established checks.
- Nginx and Nchan versions and checksums are read from `core/Makefile` and
  recorded in the release manifest.

An existing tag or image may be reused only while resuming the same draft
release for the same `release` commit and version. Source or version changes
require a new release candidate; tags and public artifacts are never retagged
or overwritten with different bytes.

## 3. Published artifacts and verification

One successful promotion produces from the same immutable inputs:

- inspected HARP package;
- `hoplite` and `hoplite-server` binaries for Apple Silicon, Intel macOS,
  x86-64 Linux, and ARM64 Linux;
- multi-platform `ghcr.io/greenways-ai/hoplite` image;
- source Homebrew formula; and
- GitHub release `SHA256SUMS` and `hoplite/release-manifest-v1` metadata.

Each executable evaluates `(+ 19 23)` and reports the release version before
upload. The promotion pulls the published OCI digest and exercises
`hoplite-server version`. It then reads the finalized GitHub release back,
checks the target commit and complete asset list, downloads every asset, and
verifies `SHA256SUMS`.

The central Homebrew tap update runs only after GitHub-release verification and
only when the protected `packages` environment exposes `HOMEBREW_TAP_TOKEN`.
It pushes and reads back the tap’s `main` SHA.

## 4. Administrator bootstrap

Before the first promotion, repository administrators must:

1. create `release` from the reviewed `main` head;
2. require pull requests and the **Release preflight / release ready** check on
   `release`, permitting merge commits for `main → release` promotion; and
3. create a `packages` environment restricted to the `release` branch and move
   `HOMEBREW_TAP_TOKEN` into that environment.

The manual promotion workflow is the only workflow that can publish a tag,
GitHub release, OCI image, or tap update. Normal branch pushes, main pushes,
and release-preflight checks do not publish artifacts.

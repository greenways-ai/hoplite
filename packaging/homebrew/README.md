# Hoplite Homebrew tap

The public tap lives at `greenways-ai/homebrew-hoplite`. Once the first tagged
release has run successfully, users install Hoplite with:

```shell
brew tap greenways-ai/hoplite
brew install hoplite
```

The release workflow builds standalone arm64 and x86_64 macOS executables,
attaches them to the GitHub release, renders `Formula/hoplite.rb` with their
real SHA-256 checksums, tests the formula, and pushes it to the tap.

## One-time tap setup

1. Create the public repository `greenways-ai/homebrew-hoplite`.
2. Add an empty `Formula/` directory or an initial README.
3. Create a fine-grained GitHub token with contents write access to that repo.
4. Add it to the Hoplite repository as the `HOMEBREW_TAP_TOKEN` Actions secret.
5. Push a tag matching `Cargo.toml`, for example `v0.1.0`.

Without the secret, release binaries are still published and the workflow
emits the rendered formula as an artifact; it simply does not push the tap.

## Local formula rendering

```shell
VERSION=0.1.0 \
ARM64_SHA256=<sha256> \
X86_64_SHA256=<sha256> \
make formula
```

Use `make audit` from this directory after placing the generated formula in a
local checkout of `homebrew-hoplite`.

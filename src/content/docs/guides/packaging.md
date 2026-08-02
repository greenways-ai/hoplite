---
title: Packaging
description: Build standalone executables and understand the Homebrew release workflow.
---

## Standalone executable

```sh
make runtime
make nginx
make macos
```

`make nginx` downloads the pinned Nginx source, verifies its checksum, and statically links the Hoplite module and Rust runtime. The final `hoplite` executable embeds that Nginx binary.

## Homebrew release

The tagged release workflow:

1. Verifies that the Git tag matches `Cargo.toml`.
2. Builds arm64 and Intel standalone macOS executables.
3. Publishes both files in a GitHub release.
4. Calculates SHA-256 checksums.
5. Generates and publishes `Formula/hoplite.rb`.
6. Updates `greenways-ai/homebrew-hoplite` when `HOMEBREW_TAP_TOKEN` is available.

The formula renderer lives under `scripts/`, and local tap instructions are in `packaging/homebrew/README.md`.

## Benchmark

```sh
make benchmark-bytecode
```

The benchmark compares HAL compilation, HBC decoding, and already-decoded execution for the bundled Hoplite namespaces. It is an engineering benchmark, not a published production performance claim.

---
title: Packaging
description: Build standalone executables and understand the Homebrew release workflow.
---

## Composable `.harp` packages

Hoplite functionality is distributed as signed Hara package archives. The
canonical package is addressed as `gh:greenways-ai:hoplite`; an activation
selects a specific export from an exact version:

```clojure
{:module/id "gh:greenways-ai:hoplite"
 :module/version "0.1.0"
 :module/export :hoplite/auth
 :module/as :auth
 :module/config {:auth/store :auth-store}}
```

A `.harp` may contain HAL sources, a `project.edn`, specs, migrations, assets,
Rust sources, and signed WASM or HTA artifacts. Rust crates are publication-time
build inputs; Hoplite activates their locked artifacts rather than compiling or
downloading native code at runtime.
`hoplite build` writes `native-adapters.edn` plus a Cargo dependency fragment.
The latter points only at verified, content-addressed installed HARP roots and
patches the addon to the host's exact ABI crate. Release builds consume that
fragment to link factory functions into the closed native registry.

Exports are independently composable. The core Hoplite archive is expected to
export `:hoplite/auth`, `:hoplite/management`, `:hoplite/gateway`, and
`:hoplite/module-runtime`. Store implementations and compatibility layers such
as PGlite, SQLite and Supabase remain separate addon packages.

Build, inspect, and install archives with the same Hara package implementation
used by Hoplite:

```sh
hoplite package build . --output target/hoplite-0.1.0.harp
hoplite package inspect target/hoplite-0.1.0.harp
hoplite package install target/hoplite-0.1.0.harp
hoplite package verify gh:greenways-ai:hoplite 0.1.0
```

Installation verifies and expands the archive into Hara's content-addressed
distribution directory. A configured exact module version must already be
installed when Hoplite starts; package activation never downloads from GitHub.

Install a published GitHub release while pinning the bytes before download:

```sh
hoplite package install gh:greenways-ai:hoplite 0.1.0 \
  --sha256 22ce8db7ea50b006813ab32d0eef211bbda469a41b6b175f19c7d111977d6075
```

The command derives the HTTPS release URL, refuses non-HTTPS transport,
verifies the supplied digest, and only then invokes the local installer.
Startup remains network-free.

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

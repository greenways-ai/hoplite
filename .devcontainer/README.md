# Hoplite development environment

The Ubuntu 24.04 devcontainer and Codex cloud share the same idempotent
bootstrap. Codex does not build this Dockerfile; it runs the setup command in
its universal image and may run it again when maintaining a cached environment.

## Setup and maintenance

```sh
bash .devcontainer/post-create.sh
```

Use these Codex environment values:

- **Setup script:** `bash .devcontainer/post-create.sh`
- **Maintenance script:** `bash .devcontainer/post-create.sh`
- **Agent internet access:** not required for the unit, example, HAL, and site checks below after setup
- **Docker integration:** run from the devcontainer or Codespaces only when `docker info` succeeds

The script reads the authoritative revision from `packaging/hara-revision`,
materializes it as sibling `../hara`, builds `hara` and `hara-test`, fetches the
locked Hoplite graph, builds the non-Nginx CLI, and installs website packages.
An existing sibling must be clean and exactly pinned; setup never resets or
replaces a mismatched checkout.

## Smoke test

```sh
hara --version
hara-test --help
hoplite version
```

## Representative offline checks

```sh
cargo +1.88.0 test --locked --manifest-path core/Cargo.toml --workspace
cargo +1.88.0 check --locked --manifest-path core/Cargo.toml --examples
npm run build --prefix website
```

## Optional Docker integration

```sh
docker info
bash packaging/scripts/smoke-cosocket-tcp.sh
```

Docker-dependent checks are unavailable in Codex cloud when the environment has
no daemon. Setup never starts Hoplite or Docker services. Ports `8080` and
`4321` are forwarded for explicit application and Astro preview commands.

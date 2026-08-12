# CI gates

Hoplite keeps required merge gates small, deterministic, and directly tied to
published library behaviour. Extension publication and downstream product
acceptance are not core health checks.

## Permanent workflows

The default branch has five permanent workflows:

| Workflow | Purpose | Merge gate |
| --- | --- | --- |
| `CI` | Library, native boundary, production integration, and docs | Yes |
| `Compatibility` | Reference extensions and minimum-version evidence | Only when its paths change |
| `HTTP and footprint benchmarks` | Reproducible performance and memory reports | No |
| `Release` | Tag-driven binaries, formula, image, and release assets | Release only |
| `pages` | Documentation deployment | No |

Temporary repair workflows must delete themselves in the commit they produce.
They are never part of the steady-state gate set.

## Required CI

### Library

The library job protects Hoplite-owned source and public boundaries:

- the reviewed Hara revision matches `packaging/hara-revision`;
- the alpha version policy matches code, documentation, and the Hara pin and
  rejects stale stable-looking application-contract identifiers;
- Rust formatting is clean;
- the committed dependency graph resolves with `--locked`;
- the complete core workspace test suite passes;
- the default executable and direct dependency boundary remains product-clean;
- minimal Rust and C embedders execute against the public runtime ABI and the
  header declarations exactly match the native symbol inventory;
- public C headers compile as C11;
- the fixed-capacity host registry, exported provider interface, and bounded
  response-source pump pass their native fixtures.

The alpha policy is implemented by
`packaging/scripts/verify-alpha-versioning.sh`. It protects the current
`hoplite.application-bundle/0-alpha` / `HAB0` envelope, Hara `HBX0` payload,
reviewed dependency pin, and the separation between portable format epochs and
native ABI generations. The full compatibility rule is documented in
[Versioning during alpha](versioning.md).

A stale lock is a source failure. CI must not hide it by removing `--locked` or
regenerating dependencies implicitly.

### Integration

The integration job protects the actual generic production path:

- the provider-neutral production image builds from reviewed source;
- two independently forced application compilations produce byte-identical,
  source-free serving artifacts;
- the final image is non-root and contains no application source, project input,
  development CLI, compiler, or build toolchain;
- `hoplite-server version` runs in the final image;
- a generic Hoplite application becomes ready;
- ordinary requests and bounded request bodies complete through Nginx;
- two intentional Nginx reloads replace the complete worker generation while
  preserving the master process, immutable HAB0/manifest/configuration bytes,
  and repeated dispatch;
- removing the serving process and starting a fresh container recreates the
  application from the same immutable source-free image;
- a three-module application proves aliases, referred Vars, namespace-local
  Vars, dependency order, and repeated prepared-handler dispatch.

`packaging/scripts/smoke-worker-reload.sh` reproduces the reload and process
recreation check against an already built image:

```sh
bash packaging/scripts/smoke-worker-reload.sh hoplite-ci
```

This job must use a generic application. It must not require a database,
application identity model, provider release, or another repository.

### Documentation

The documentation job installs from the committed lock and builds the website.
Examples and public API descriptions are therefore checked in the same pull
request that changes them.

## Compatibility evidence

`Compatibility` is intentionally separate from core CI. It may test:

- reference host-service implementations;
- supported minimum Rust versions for isolated ABI packages;
- compatibility fixtures for an extension-facing interface;
- old-format readers during an announced migration window.

It must not publish packages, download mutable “latest” artifacts, edit a
branch, or become a universal gate for changes that do not touch the relevant
interface.

When an extension matures into a product, its build, release, and durability
matrix moves with that extension. Hoplite retains interface conformance, not the
product's operational release train.

## Benchmark evidence

Benchmarks run manually, on a schedule, and for release tags. They publish:

- HTTP throughput and latency with machine and concurrency metadata;
- executable and image sizes;
- one-worker and marginal multi-worker memory;
- source compilation, bytecode decoding, and application-bundle startup costs.

Noisy wall-clock measurements do not block ordinary pull requests. A benchmark
becomes a required gate only after it has a stable fixture, a justified budget,
and demonstrated low variance.

## Release and deployment

- Normal branch and main pushes never create GitHub releases.
- `Release` runs only for version tags or an explicit rebuild of an existing
  tag.
- A pre-1.0 package tag does not promote an alpha portable contract to a stable
  major version.
- Provider or downstream product artifacts are not published by Hoplite core
  workflows.
- `pages` may deploy documentation from main and validated benchmark evidence.

## Gate design rules

A required check must satisfy all of these rules:

1. It protects a documented Hoplite API, ABI, runtime invariant, or supported
   build target.
2. It is deterministic enough that a failure means the proposed change is not
   ready.
3. It has one owner and one implementation; duplicate workflows are removed.
4. It fails with the violated contract visible near the end of the log.
5. It can run without credentials for a downstream product.
6. It neither commits generated changes nor patches the pull-request branch.
7. It uses behavioural tests instead of source-text greps whenever behaviour can
   be exercised directly.

## Changing the gate set

A pull request that adds or promotes a gate must document:

- the public surface it protects;
- the failure class it detects that existing jobs miss;
- expected runtime and external dependencies;
- why path filtering or scheduled evidence is insufficient;
- how a contributor reproduces the check locally.

A gate that repeatedly fails for unrelated infrastructure reasons is demoted or
repaired; it is not allowed to become permanent background noise.

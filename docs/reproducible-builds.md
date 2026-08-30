# Reproducible application builds

Hoplite production startup consumes generated, source-free application output.
For the same Hoplite revision, reviewed Hara source revision, reviewed Hara
Native revision, project source, project configuration, build mode, and
dependency inputs, the generated `.hoplite` directory must be byte-identical.

The permanent integration gate proves this by compiling the multi-module fixture
twice. Each compilation receives a distinct cache nonce at the application-build
layer, so Docker cannot reuse the previous generated application. The expensive
reviewed Rust, Hara source, Hara Native, and Nginx toolchain layers remain
cacheable and are not the subject of the comparison.

## Exported evidence target

`packaging/docker/Dockerfile` exposes the named target
`application-artifacts`. It contains only the contents of `/app/.hoplite` from
the builder stage. It does not contain application source, the Hoplite compiler,
Cargo, Rust, Nginx build input, or the surrounding runtime image.

The target is evidence plumbing rather than a separately published product. A
normal Docker build still selects the final source-free `hoplite-server` image.

## Required generated output

Each independent build must contain at least:

- `app.hbx`, the Hara HBX0 namespace bundle carrying ordered HBC0 modules;
- `apps.hta`, the exact prepared application/route manifest;
- `conf/nginx.conf`, the generated production Nginx configuration.

No `.hal`, `project.edn`, or `hara.extension.edn` source input may appear in the
exported directory.

## Comparison law

The integration job uses unique values of
`HOPLITE_REPRODUCIBILITY_NONCE` for the two builds. That argument exists only to
invalidate the application compilation layer. It is deliberately present in the
build environment: if the compiler accidentally incorporates unrelated ambient
environment into output, the comparison fails.

For both exported trees, CI:

1. checks the required generated files and source exclusion;
2. computes a sorted relative-path SHA-256 manifest for every regular file;
3. compares those manifests;
4. recursively compares all exported bytes and filesystem entries.

A file-name difference, missing file, checksum difference, generated timestamp,
random identifier, unstable dependency order, machine path, or cache-dependent
byte changes therefore fail the required `CI / integration` job.

## Scope

This proves application-output reproducibility within one Linux builder image
and one immutable Hoplite/Hara source/Hara Native revision triple. Hara source
is mounted only to compile the application; the final server receives the
resulting HBX0 and has no source checkout. It does not yet prove that different
operating systems or compiler toolchains produce identical artifacts. Cross-host
reproducibility, retained build provenance, and independently repeated tagged
release builds remain separate release-quality work.

The source-free final image assertion and real Nginx serving fixtures remain
independent checks. Reproducible bytes are necessary, but they do not by
themselves prove route correctness, runtime ownership, suspension, streaming,
cancellation, or disconnect behaviour.

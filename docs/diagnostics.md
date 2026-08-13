# Inspecting a built Hoplite application

`hoplite inspect` reads generated `.hoplite` output without discovering, parsing,
compiling, or executing application source.

```shell
hoplite inspect [--json] [--show-paths] [--manifest FILE] [PROJECT|OUTPUT|BUNDLE]
```

The command applies the same bounded HAB0 and exact-manifest validation used by
`hoplite verify`, then reports:

- the `hoplite.application-bundle/0-alpha` identity and runtime ABI;
- exact bundle, manifest, generated configuration, and platform artifact sizes
  and SHA-256 digests;
- the embedded HBX0 byte count;
- application, route, and adapter counts from the validated route manifest;
- the number of generated OpenAPI documents;
- whether the inspected output contains application source inputs.

Human-readable output is the default. `--json` emits
`hoplite.inspect/0-alpha`, a public alpha report whose incompatible changes
require a new epoch and migration note.

## Redaction

Filesystem paths are omitted from successful human and JSON output by default.
Use `--show-paths` when local path disclosure is intentional. File failures
report stable classes and I/O categories without embedding build-machine paths.
Source-free failures list only paths relative to the generated output directory.

The command never prints source text, credentials, signatures, native pointers,
or provider internals.

## Required and optional artifacts

`app.hbx` and its exact `apps.hta` are required and validated before a report is
emitted. Generated Nginx configuration, platform HTA/EDN, and OpenAPI documents
are reported as present or absent so an operator can inspect a partial build
without executing it. Invalid or incompatible required inputs exit non-zero.

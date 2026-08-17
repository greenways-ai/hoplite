# Embedding the Hoplite runtime

Hoplite exposes one worker-local native runtime through
[`hoplite_runtime.h`](../core/nginx/hoplite_runtime.h). The same implementation
is available to Rust embedders through the `hoplite-runtime` crate.

The executable fixtures are:

- [`core/runtime/examples/embed.rs`](../core/runtime/examples/embed.rs), a Rust
  host using the exported runtime functions directly;
- [`core/runtime/examples/embed.c`](../core/runtime/examples/embed.c), a C host
  compiled against the public header and linked to the built static library.

Both fixtures create a runtime, bootstrap a small Hara namespace, prepare one
handler, read and close the response, close the prepared handler, and free the
runtime. The Rust fixture retains the portable V2 request path. The C fixture
constructs a V4 borrowed raw-field descriptor and proves that a `:raw` handler
receives the supplied scheme.

## Runtime ownership

A runtime belongs to one worker or thread for its complete lifetime. Runtime,
handler, work, request, response, body, and buffer handles are opaque and local
to that runtime. They must not be shared with another runtime or used after the
owning object has been closed.

Input slices are borrowed only for the duration of the call unless the API
explicitly transfers ownership. A non-null V3 request-body descriptor transfers
exclusive lifecycle ownership after top-level pointer preflight. A V4 raw
descriptor is borrowed instead: its callback and context remain valid through
the active request, including suspension, and the runtime copies only the
descriptor. Runtime buffers are released only with `hoplite_buffer_free`, using
the exact pointer and length returned by Hoplite.

## Minimal lifecycle

The smallest synchronous embedding flow is:

1. require `hoplite_abi_version() >= 5` when constructing V4 raw requests;
2. allocate one runtime with `hoplite_runtime_new`;
3. bootstrap modules for development, or bootstrap one verified HAB0 application
   and exact manifest for production;
4. prepare a qualified handler or use a prepared application route;
5. invoke with a borrowed V2 request, transferred V3 body descriptor, or V4
   request carrying the optional borrowed raw descriptor;
6. read the completed response while its response handle remains live;
7. close response and handler handles;
8. free the runtime.

Production embedders should use
`hoplite_bootstrap_application_v1` or
`hoplite_bootstrap_application_files_v1`. `hoplite_bootstrap_modules` is a
source-development compatibility path and is intentionally absent from the
source-free production startup contract.

## Running the fixtures

From the repository root:

```sh
bash packaging/scripts/smoke-embedding.sh
```

The script runs the Rust example, builds the static runtime, obtains Rust's
required native static-library link set, compiles and executes the C example,
and compares the public symbol inventory against both the C header and the
actual static library.

## Native symbol contract

[`native-symbols.txt`](native-symbols.txt) is the exact public `hoplite_*`
symbol set for the current native ABI generations. The required library gate
fails when:

- the header declares an unclassified native function;
- the header loses a listed public function;
- the built static library exports an additional `hoplite_*` symbol;
- the built static library no longer exports a listed symbol;
- either embedding fixture fails its lifecycle or response assertions.

A changed C call shape receives a new versioned symbol. Existing versioned
symbols are not repurposed in place. Portable HAB0/HBX0/HBC0 identities remain a
separate version space from these native symbol generations.

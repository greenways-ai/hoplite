# Cosocket lifecycle testing

The runtime includes a deterministic model-based lifecycle test for the
source-free TCP/Unix/resolver/pool/backlog state machine. It uses the checked-in
seed corpus at
`core/runtime/tests/fixtures/cosocket_lifecycle_seeds.txt`, runs a bounded
operation count in required CI, and prints the seed and normalized operation
trace when an invariant fails.

Run the required corpus with:

```sh
cargo test --manifest-path core/runtime/Cargo.toml \
  --test cosocket_lifecycle_model -- --nocapture
```

Run the larger local-only stress corpus by setting both bounds and enabling
ignored tests:

```sh
HOPLITE_COSOCKET_STRESS_SEEDS=1024 \
HOPLITE_COSOCKET_STRESS_STEPS=4096 \
cargo test --manifest-path core/runtime/Cargo.toml \
  --test cosocket_lifecycle_model -- --ignored --nocapture
```

The model treats network outcomes as ordinary results and invalid descriptors,
foreign ownership, and illegal transitions as structured failures. Teardown is
checked after every generated trace and after client abort or worker reload:
native descriptors, timers, resolver contexts, backlog waiters, pool entries,
cleanup registrations, buffers, and promises must all be retired exactly once.
The model complements the real-Nginx source-free cosocket fixtures; it does not
replace them or alter production socket semantics.

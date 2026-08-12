# Multi-module production fixture

This is a provider-neutral Hoplite application used only to prove the production
bundle and worker contract.

It contains three application namespaces:

1. `hoplite.fixture.foundation` owns a namespace-local Var and an aliased
   function.
2. `hoplite.fixture.composition` consumes the first namespace through both
   `:as` and `:refer`, then owns another Var.
3. `hoplite.fixture.application` consumes the second namespace through both
   forms and exposes `/hello`.

A successful production request returns:

```text
alias|foundation|composition|composition|application
```

That body can only be assembled when Hara preserves dependency order, aliases,
referred Vars, namespace-local Vars and cross-namespace calls through HBB2,
HAB1, isolated preflight, staged worker publication and Nginx dispatch.

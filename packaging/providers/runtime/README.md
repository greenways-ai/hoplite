# Blob and value provider set

`provider-set-lock.json` is the first closed multi-provider distribution lock.
It requires the exact published `hoplite.blob` and `hoplite.value` packages as
one compatible set without adding either implementation to generic Hoplite
core.

```text
hoplite.blob  0.1.1
  sha256:03c5dea9854cf23b60c7d2638c17712accc7e77eb53db4d15ed0b45327ee8210

hoplite.value 0.1.0
  sha256:47e96af3768621b25ef448004795ce9ecbdca091cfa31910308009156ed89e4f

hoplite.value
  -> hoplite.blob
  -> hoplite-blob-filesystem-reader/0.1.0
```

The set lock repeats only provider names, versions, archive digests and the
logical object-backend binding. Each provider's own lock remains authoritative
for its repository, release tag, asset, source revision and media type.

The validator rejects:

- a missing provider;
- duplicate or unknown providers;
- extra or missing bindings;
- provider version or digest drift;
- a backend package or package-version mismatch;
- a value binding that disagrees with the validated object-backend lock.

The distribution workflow additionally downloads both exact release archives,
verifies their closed inventories and byte-compares the immutable reader carried
by both packages.

This profile does not include `hoplite.store`. Mutable CAS state remains an
independent package and can be added only through a later explicit profile
version. The set lock is trusted distribution input and never enters portable
requests, HAL values or Nginx request handling.

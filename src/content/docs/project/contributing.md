---
title: Contributing
description: Build, test, inspect, and contribute to Hoplite.
---

## Prepare the repositories

```sh
git clone https://github.com/hara-lang/hara.git hara.lang
git clone https://github.com/greenways-ai/hoplite.git hoplite
cd hoplite
make setup
```

## Run checks

```sh
make check
make runtime
make nginx
make macos
```

`make nginx` downloads and verifies the pinned Nginx source. Use the narrowest relevant target while iterating and run the full checks before proposing a change.

## Exercise the example

```sh
make example-check
make example-build
make example-dev
curl -i http://localhost:8080/hello
```

## Report and discuss work

Use [GitHub issues](https://github.com/greenways-ai/hoplite/issues) for reproducible bugs, compatibility problems, documentation corrections, and focused feature proposals. Include the platform, Hoplite and Hara revisions, command, expected behavior, and observed output.

Hoplite source is licensed under the Eclipse Public License 2.0.

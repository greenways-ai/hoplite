---
title: Authentication
description: Bootstrap management access and understand Hoplite realms, device keys, sessions, and provider packages.
---

Hoplite authenticates both management users and application callers. Identity
is therefore available before application modules run and cannot be replaced by
a package loaded into the application.

## Bootstrap management access

Generate an Ed25519 key on the administrator device, then initialize the node:

```sh
hoplite auth init PROJECT
hoplite auth enroll BOOTSTRAP_TOKEN ED25519_PUBLIC_KEY_HEX PROJECT
```

The bootstrap token is stored only as a SHA-256 digest, can be used once, and
expires after 15 minutes. The SQLite control database and its containing
directory are created with owner-only permissions on Unix systems.

For containers, persist the store separately from application data:

```sh
docker run --rm -p 8080:8080 \
  -v hoplite-control:/var/lib/hoplite \
  ghcr.io/greenways-ai/hoplite:latest
```

## Realms and principals

Every profile declares separate `:management` and `:application` realms. The
management realm is always protected. A successful request produces an
immutable principal contract containing the subject, realm, session, device,
and claims. Gateway integrations must discard caller-supplied identity headers
before attaching that principal to the Hara request context.

## Session behavior

The built-in key provider sends a random, expiring challenge for the device to
sign. A verified challenge produces opaque access and refresh tokens:

- only token digests are stored in `control.db`;
- access tokens are short lived and bound to one realm;
- refresh tokens are single-use and rotated;
- reuse outside the configured grace interval revokes the session;
- logout and management revocation invalidate access immediately;
- enrollments, sessions, rotations, reuse, and revocations create audit events.

## Provider packages

WebAuthn and GitHub/OIDC support belong in signed Hara packages declaring the
privileged `:hoplite/auth-provider` capability. Providers prove or link an
external credential, but Hoplite still creates sessions, emits principals, and
enforces realm boundaries. Provider secrets use deployment-time secret
references and must not be committed to `project.edn`.

## Management API

Start the loopback-only API from a configured Hoplite project:

```sh
hoplite auth serve --listen 127.0.0.1:9090 PROJECT
```

| Method and path | Authentication | Purpose |
| --- | --- | --- |
| `GET /health` | Public, loopback only | Process health |
| `POST /v1/auth/enroll` | Bootstrap token | Enroll the first management device |
| `POST /v1/auth/challenges` | Enrolled public key | Create an Ed25519 challenge |
| `POST /v1/auth/sessions` | Signed challenge | Issue a management session |
| `POST /v1/auth/refresh` | Refresh token | Rotate a session token pair |
| `GET /v1/auth/me` | Management bearer token | Inspect the current principal |
| `POST /v1/auth/revoke` | Management bearer token | Revoke a session immediately |

Requests and responses use JSON. Requests are limited to 64 KiB and responses
include `Cache-Control: no-store`. The server refuses wildcard and non-loopback
bindings; deliberate remote administration should be placed behind a separate
authenticated transport such as an SSH tunnel.

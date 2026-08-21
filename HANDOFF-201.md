# Handoff: PR #201 — Bounded cosocket connect backlog

## Current state (2026-08-22)

- **Hara bootstrap issue is fixed.** The production failure
  `hoplite.rtc: namespace declaration: Cannot require missing namespace: std.stream.duplex`
  was caused by a stale `core/rust/assets/std.foundation.hbx` artifact in Hara.
  - `hara-lang/hara#994` regenerated the artifact to include `std.stream.duplex`.
  - `greenways-ai/hoplite#207` advanced Hoplite's pinned Hara revision to `4e19805cd`.
- **PR #201 has been rebased** onto the updated `main` (head `81e26869`).
- **CI status on PR #201:**
  - `docs` ✅
  - `library` ✅
  - `integration` ✅
  - `TCP cosocket` ❌

## Remaining blocker

The `TCP cosocket` job now fails with:

```
numeric TCP keepalive did not reuse one persistent connection: 11:1|0 / 12:1|0
```

This failure also occurs on `main` (run at `ecda211`), so it is a
**pre-existing cosocket keepalive-reuse issue**, not a regression introduced by
PR #201's bounded-backlog changes.

## Likely cause / where to look

The cosocket fixture `packaging/fixtures/cosocket-tcp/src/hoplite/cosocket/application.hal`
connects with `{:pool pool-name :pool-size 2 :backlog 0}`, sends/receives, then calls
`socket/setkeepalive`. The next request should find the idle connection in the
worker-local pool (pool key + reuse count), but it is opening a new connection
instead.

Primary suspect: `core/nginx/cosocket/hoplite_cosocket_pool.inc`
- `hoplite_cosocket_pool_setkeepalive` stores the idle entry.
- `hoplite_cosocket_pool_connect` / checkout path searches the idle list.
- The idle entry may be closed, not matched by key, or evicted before reuse.

## Suggested next steps

1. Reproduce the failure locally or inspect the CI run logs:
   - Run: `gh run view <run-id> --repo greenways-ai/hoplite --job <tcp-cosocket-job-id> --log-failed`
   - Search for `did not reuse one persistent connection`.
2. Add targeted logging around idle-pool insert/lookup in
   `hoplite_cosocket_pool.inc` and push a debug branch.
3. Verify whether the issue is:
   - idle entry closed by timer/peer before second request,
   - key mismatch between `setkeepalive` and the second `connect`, or
   - pool capacity eviction (`hoplite_cosocket_pool_make_capacity`).
4. Once the keepalive bug is fixed on `main`, rebase PR #201 again; it should
   then be able to go green.

## Branches / PRs involved

- `agent/cosocket-bounded-backlog` — PR #201 (rebased, pushed, draft)
- `main` — currently red on `TCP cosocket` keepalive reuse
- `hara-lang/hara#994` — regenerated `std.foundation.hbx` (merged)
- `greenways-ai/hoplite#207` — Hara revision bump to `4e19805cd` (merged)

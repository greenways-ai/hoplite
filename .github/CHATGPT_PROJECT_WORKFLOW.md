# ChatGPT GitHub connector programming workflow

**Repository:** `greenways-ai/hoplite`

**Tracking issue:** `#208`

**Initial base:** `ecda211ad117d047081a6bd9075a19f3c87c9669`

**Status:** canonical repository-local contract for programming initiated through the ChatGPT web application

This contract specializes the organisation's
[connector-first delivery workflow](https://github.com/greenways-ai/.github/blob/main/docs/connector-first-delivery.md).
GitHub is the execution host and durable record: issues define the work,
branches and commits carry it, draft pull requests expose it, and Actions
provide executable evidence. An MCP server is not part of this workflow.

## Delivery loop

1. Read `AGENTS.md`, this file, the executable issue, linked relationships, and
   any more-specific repository instructions.
2. Resolve the current default branch and record its exact commit SHA before
   editing. Do not silently retarget an in-flight task to a newer base.
3. Create or reuse an executable issue with Outcome, Scope, Acceptance criteria,
   Validation, Relationships, Readiness, and Delivery.
4. Create `agent/<issue>-<slug>` from that recorded base. Inspect the intended
   file set and keep unrelated work out of the branch.
5. Implement product code and its tests together. Never commit generated caches,
   local credentials, build outputs, or transient evidence.
6. Run the repository-native checks selected below and commit the bounded change.
7. Push through the GitHub connector, read the remote branch back, and verify
   that its head is the intended SHA.
8. Treat the branch run of `Connector code execution` as push preflight. Repair
   failures before opening the draft pull request.
9. Open or reuse one draft pull request for the branch. Read it back and verify
   repository, state, base, head branch, and head SHA.
10. Inspect every relevant Actions run, including the repository's existing
    authoritative workflows. Repair on the same branch until required checks
    are green. Do not create a repository-mutating Actions workaround.
11. Mark ready only when explicitly requested and all required checks are green.
    Report the issue URL, branch, exact commit SHA, pull-request URL, and
    relevant run URLs.

## Path-aware execution

`./scripts/connector/detect-code-scope` compares the exact base to `HEAD`.
Documentation-only changes may select no product scope. Unknown executable or
configuration paths fail toward all scopes so new code cannot escape execution.

| Scope | Representative paths | Committed command |
|---|---|---|
| `runtime` | `core/**`, Rust/C ABI and runtime inputs | `scripts/connector/run-code runtime` |
| `contracts` | `packaging/**`, pinned Hara and product contracts | `scripts/connector/run-code contracts` |
| `website` | `website/**` | `scripts/connector/run-code website` |

The selector's `--self-test`, shell syntax checks, and the final evidence gate
always run. The workflow uses `contents: read`; it cannot commit, push, label,
merge, release, deploy, or mutate an issue or pull request.

## Existing checks remain authoritative

The existing `CI` workflow remains the merge authority for `library`, `integration`, and `docs`, including production-image and serving-plane proofs.

The connector workflow deliberately reuses the reviewed Hara revision from `packaging/hara-revision`; it never advances that pin.

## Evidence contract

Each selected scope uploads a small text artifact containing repository, scope,
base, head, run ID, attempt, and the exact committed command. The final
`evidence-gate` accepts only `success` or `skipped` scope results and requires
the selector contract to succeed.

The artifact is supporting evidence, not a substitute for the GitHub branch,
commit, draft pull request, or the repository's normal required checks.

# AGENTS.md

## Connector-first delivery

GitHub issues, pull requests, native relationships, checks, and repository
documents are authoritative. GitHub Projects are visual projections of that
state, not a separate source of truth.

Use the organisation workflow in
[greenways-ai/.github](https://github.com/greenways-ai/.github/blob/main/docs/connector-first-delivery.md).
Before implementing an issue, read its relationships and linked pull requests,
then follow this repository's local documentation and validation instructions.

Every executable issue must define Outcome, Scope, Acceptance criteria,
Validation, Relationships, Readiness, and Delivery. Keep durable decisions and
progress in the issue or pull request so that they remain visible through the
GitHub connector; do not rely on chat history as the only record.

## GitHub publication contract

These rules are mandatory whenever a user asks to open, create, raise, or publish a pull request; push a branch; or implement changes and publish them to GitHub.

A requested pull request is a fail-closed workflow. Editing files, running tests, creating a local commit, producing a patch, or generating a report does not complete the task.

### Verified definition of done

Do not say `published`, `pushed`, `opened`, `created`, or `complete` unless the corresponding operation has been verified. A pull request is complete only after all of the following are true:

1. The intended changes are committed.
2. The commit exists on a remote GitHub branch.
3. The remote head SHA equals the intended commit SHA.
4. GitHub returned a real pull request number and canonical URL.
5. The pull request was fetched back from GitHub.
6. The read-back matches the expected repository, open state, base branch, head branch, and head SHA.

A local diff, local commit, branch name, patch, report, HTML redirect, or `sandbox:/mnt/data/...` artifact is never proof that a GitHub pull request exists.

### Required publication workflow

1. Resolve the exact repository and current default branch. Read this file and any more-specific `AGENTS.md` files before editing.
2. Inspect `git status`, the complete diff, and the intended file set. Never stage unrelated user work.
3. Start from the current default branch unless the user specified another base. Use a task branch such as `agent/<description>`.
4. Run the relevant repository validation and record the commands and outcomes.
5. Commit only the intended changes and record the commit SHA.
6. Push the branch to the correct GitHub remote.
7. Verify that the remote branch exists and resolves to the exact intended SHA. `git ls-remote` or an equivalent GitHub branch/commit read is acceptable.
8. Create the pull request using the connected GitHub pull-request action. Authenticated `gh pr create` is an acceptable fallback.
9. Fetch the created pull request back using a connected GitHub read action or `gh pr view --json number,url,state,isDraft,title,headRefName,headRefOid,baseRefName`.
10. Verify the repository, PR number, open state, base branch, head branch, and head SHA against the values recorded above.
11. Return the exact canonical URL supplied by GitHub.

Before creating a new pull request, check whether the head branch already has an open pull request. Reuse and update the matching pull request rather than creating a duplicate.

### URL rules

A successful result must use the exact canonical URL returned by GitHub:

```text
https://github.com/OWNER/REPOSITORY/pull/NUMBER
```

Do not:

- escape the scheme as `https\://`;
- invent or guess a pull request number;
- manually append `/changes`, `/files`, or another suffix;
- replace the GitHub URL with a sandbox link;
- create an HTML redirect as a substitute for a pull request.

Sandbox reports may be supplemental, but the verified GitHub URL must be the primary result.

### Failure behavior

If checkout, validation, commit, push, remote-SHA verification, pull-request creation, or pull-request read-back fails:

1. Stop claiming publication success.
2. State the last successful stage.
3. State the exact failing stage and relevant error.
4. Clearly distinguish local uncommitted work, a local commit, a remotely pushed branch, and a verified GitHub pull request.
5. Do not use success words for operations that were not verified.

Use these exact summaries when applicable:

> Changes were committed locally but were not published to GitHub.

> The branch was pushed, but no verified GitHub pull request was created.

### Multi-repository and submodule work

For work spanning multiple repositories:

- use a separate branch, commit, and pull request in each repository;
- verify every pull request independently;
- return every canonical pull-request URL;
- do not describe the overall train as complete while any repository remains unverified;
- do not update workspace submodule pins to commits that have not been merged into the child repositories, unless the user explicitly requests a stacked unmerged-pin workflow.

### Required final report

For every successfully published pull request, report:

```text
Pull request: <exact canonical GitHub URL>
Repository: <owner/repository>
PR: #<number>
State: <draft or ready>
Head: <branch> @ <verified SHA>
Base: <base branch>
Validation: <commands actually run>
```

## ChatGPT GitHub connector programming workflow

Programming tasks initiated through the ChatGPT web application must follow
[`.github/CHATGPT_PROJECT_WORKFLOW.md`](.github/CHATGPT_PROJECT_WORKFLOW.md).
Use the issue, task branch, commit, draft pull request, Actions runs, and repair
history as the durable execution record.

The committed execution contract is
[`.github/workflows/connector-code-execution.yml`](.github/workflows/connector-code-execution.yml)
together with [`scripts/connector/`](scripts/connector/). It is read-only with
respect to repository contents. Do not create a one-off repository-mutating
workflow as a substitute for connector delivery. Product release, deployment,
materializer, and agent-adapter workflows remain domain-specific and are not
delivery shortcuts.

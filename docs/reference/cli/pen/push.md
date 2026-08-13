# ++"nave pen push"++

Push the apply branch's committed local tip, verifying evidence before reporting `ok`. The
third apply-mode verb — runs after `pen commit`. Distinct from `pen exec --push-changes`,
which pushes whatever the working tree happens to hold rather than a verified, sidecar-tracked
commit.

## Usage

```bash
--8<-- "docs/_snippets/cli/pen/push.txt"
```

## Request file

```json
{
  "protocol_version": 1,
  "repos": [{"repo": "acme/docs"}]
}
```

Just the repo identity — `push` reads the committed SHA and origin URL from the sidecar
`pen commit` wrote.

## What it does

Per repo:

1. Verifies `refs/heads/<BRANCH>` locally still equals the recorded commit SHA (`diverged` if
   not).
2. Verifies `origin`'s URL is unchanged since provisioning (`push-rejected` if not).
3. Pushes with an explicit fully-qualified refspec
   (`refs/heads/<BRANCH>:refs/heads/<BRANCH>`) — idempotent: re-pushing identical history is a
   no-op fast-forward.
4. Reads back `remote`, `remote_ref`, `remote_sha` (via `origin/<BRANCH>`, which git updates
   locally right after a successful push), and `upstream`. Any of these failing to read is
   treated as a push-evidence failure, not a silent `ok` with holes in it.

## Output

```json
{
  "protocol_version": 1,
  "adapter_state": "ok",
  "repos": [
    {
      "repo": "acme/docs",
      "remote": "https://github.com/acme/docs.git",
      "remote_ref": "pulse/apply/p1",
      "remote_sha": "b2c3d4...",
      "upstream": "origin/pulse/apply/p1",
      "local_commit_sha": "b2c3d4...",
      "state": "ok"
    }
  ]
}
```

Per-repo `state`: `ok`, `missing-branch`, `diverged`, `push-rejected`, `no-apply-state` (no
prior `pen commit` recorded), `unknown-repo`.

## Example

```bash
nave pen push nave/lowest-direct pulse/apply/p1 --request push-request.json --json
```

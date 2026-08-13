# ++"nave pen reset"++

Discard a partial apply attempt: CAS-guarded local and remote branch cleanup. The terminal
apply-mode verb — call after a successful landing (to tidy up) or after any failure in
`branch`/`commit`/`push` (to unwind).

## Usage

```bash
--8<-- "docs/_snippets/cli/pen/reset.txt"
```

## Request file

```json
{
  "protocol_version": 1,
  "repos": [
    {"repo": "acme/docs", "expected_pushed_sha": "b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3"}
  ]
}
```

`expected_pushed_sha` is optional — omit it (or pass `null`) when the branch was never pushed;
`reset` then only cleans up locally and never touches the remote.

## What it does

Per repo:

**Local** (always attempted, idempotent): discards any dirty state
(`reset --hard` + `clean -fd` — a no-op on an already-clean tree), checks out the pen's own
branch if the apply branch is currently checked out (every real pen clone has its own branch
checked out by `pen create`), then deletes the local apply branch. A checkout failure is
reported, never silently discarded.

**Remote** (only when `expected_pushed_sha` is given): first verifies the origin remote's push
URL is unchanged since `pen branch` provisioned it (`remote-deleted: false` and
`state: "evidence-mismatch"` if not — this is a push, so it uses whichever URL `git push`
would actually use, `remote.origin.pushurl` when configured, not just the fetch URL). Then
probes `git ls-remote` for the branch: a **confirmed-empty** result (the probe itself
succeeded and found nothing) is treated as an idempotent success — the branch is already gone,
nothing to do. Anything else (the branch present, or the probe itself failing — network,
auth, ...) falls through to the actual deletion: a single atomic
`git push --force-with-lease=<ref>:<expected> origin :<ref>`, never a separate "read the SHA,
then delete" pair (which would leave a window for another actor to replace the branch between
the read and the delete). `--force-with-lease` performs the SHA comparison and the delete as
one server-side operation, so a failed/unreachable probe never gets silently reported as
success — only a *confirmed* absence short-circuits.

## Output

```json
{
  "protocol_version": 1,
  "adapter_state": "ok",
  "repos": [
    {"repo": "acme/docs", "local_reset": true, "remote_deleted": true, "state": "ok"}
  ]
}
```

Per-repo `state`: `ok`, `remote-cas-mismatch` (the remote branch moved since it was pushed —
left intact, not deleted), `evidence-mismatch` (the origin push URL changed since
provisioning — left intact, not deleted), `missing-branch` (a local cleanup step failed),
`unknown-repo`.

## Example

```bash
nave pen reset nave/lowest-direct pulse/apply/p1 --request reset-request.json --json
```

# ++"nave pen commit"++

Bounded-stage and commit dirty apply-branch paths, with post-exec invariant checks. The second
apply-mode verb — runs after `pen branch` has provisioned the branch and after whatever
transform produced the dirty working tree (typically `pen exec`).

## Usage

```bash
--8<-- "docs/_snippets/cli/pen/commit.txt"
```

## Request file

```json
{
  "protocol_version": 1,
  "repos": [
    {"repo": "acme/docs", "paths": ["package-lock.json"]}
  ]
}
```

No `expected_base_sha` and no `message` in the request body — the base SHA is checked against
the sidecar `pen branch` wrote, and the commit message is the `-m`/`--message` flag.

## What it does

Per repo, before staging anything:

1. **Branch still checked out** — the apply branch (the `<BRANCH>` argument) must still be
   `HEAD`'s symbolic ref.
2. **`HEAD` unchanged since provisioning** — must still equal the base SHA `pen branch`
   recorded; a moved `HEAD` means something committed during the transform step.
3. **`origin` remote unchanged** — the remote URL must match what `pen branch` recorded.
4. **Dirty paths within bounds** — every path `git status` reports dirty must be in the
   request's `paths` list; anything outside fails the commit closed as `dirty-outside-bounds`,
   never `git add -A`.

If all four hold: stages exactly the requested `paths` (each individually rejected if it's
absolute, contains `..`, or falls under `.git/`), commits with `core.hooksPath=/dev/null` (so
nothing planted in `.git/hooks` by an earlier step fires), then re-verifies the committed diff
touched only the bound paths before reporting `ok`.

## Output

```json
{
  "protocol_version": 1,
  "adapter_state": "ok",
  "repos": [
    {"repo": "acme/docs", "local_commit_sha": "b2c3d4...", "state": "ok"}
  ]
}
```

Per-repo `state`: `ok`, `nothing-to-commit` (no dirty paths — not an error), `dirty-outside-bounds`,
`invariant-violated`, `missing-clone`, `no-apply-state` (`pen branch` was never run for this
apply branch), `unknown-repo`.

## Example

```bash
nave pen commit nave/lowest-direct pulse/apply/p1 \
  --request commit-request.json -m "chore: bump lockfile" --json
```

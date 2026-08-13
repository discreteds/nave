# ++"nave pen branch"++

Provision the apply branch across a request's repos off a verified remote base. The first of
the four apply-mode mutation verbs (`branch` → `commit` → `push` → `reset`) — see
[Pens](../../../concepts/pens.md) for how apply mode fits the pen model.

## Usage

```bash
--8<-- "docs/_snippets/cli/pen/branch.txt"
```

## Request file

```json
{
  "protocol_version": 1,
  "apply_ref": "pulse/apply/p1",
  "repos": [
    {
      "repo": "acme/docs",
      "base_ref": "develop",
      "expected_base_sha": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"
    }
  ]
}
```

`apply_ref` is one branch name for the whole request — every repo in a single `branch` call
provisions the same apply branch. Per repo: `base_ref` names the remote branch to provision
off; `expected_base_sha` is a compare-and-swap guard against a stale caller-side view of that
branch.

## What it does

Per repo:

1. Fetches `base_ref` **fresh from `origin`** — never trusts a locally cached ref.
2. Compares the observed remote SHA to `expected_base_sha`. A mismatch is reported as
   `stale-base`, not a hard error — the caller re-derives and retries.
3. Verifies the resolved object is a commit (`not-a-commit` if not).
4. Fails closed with `exists` if `apply_ref` already exists locally — branch reuse is a
   caller-level decision (via a crash-recovery journal), never assumed here.
5. Checks out `apply_ref` off the verified SHA and records the provisioned base (and the
   `origin` remote URL) in a sidecar under the pen's `apply/` directory, for `commit` to
   verify against later.

## Output

```json
{
  "protocol_version": 1,
  "adapter_state": "ok",
  "repos": [
    {
      "repo": "acme/docs",
      "base_ref": "develop",
      "expected_base_sha": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
      "observed_base_sha": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
      "apply_ref": "pulse/apply/p1",
      "state": "ok"
    }
  ]
}
```

Per-repo `state`: `ok`, `stale-base`, `exists`, `missing-ref`, `not-a-commit`, `unknown-repo`
(the requested repo isn't part of this pen), `evidence-unavailable` (checkout succeeded but
the origin remote's fetch/push URLs couldn't be captured for the sidecar `commit`/`push` rely
on — never reported as `ok`). Envelope `adapter_state` is `ok` whenever the
command ran and produced a determinate result for every repo — even if some repos report a
controlled failure state. `error` is reserved for request-level failures (bad
`protocol_version`, malformed JSON, invalid ref/sha syntax); an `error` envelope carries a
top-level `reason` and an empty `repos` array, and the process exits non-zero.

## Example

```bash
nave pen branch nave/lowest-direct --request branch-request.json --json
```

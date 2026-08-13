# ++"nave pen"++

Operations on pens — named subsets of the fleet.

## Usage

```bash
--8<-- "docs/_snippets/cli/pen.txt"
```

See [Pens](../../concepts/pens.md) for the concept.

## Planned (🚧 not yet shipped)

The following are designed and described in the
[orchestration essay](https://cog.spin.systems/fleet-ops-orchestrating-codemods)
but not yet available in the CLI:

[++"nave pen open"++](pen/open.md)

- [++"nave pen run"++](pen/run.md) — apply a declarative codemod + push branches.
- [++"nave pen open"++](pen/open.md) — create PRs (wrapping `gh pr create`).
- [++"nave pen merge"++](pen/merge.md) — merge PRs (wrapping `gh pr merge`).
- [++"nave pen close"++](pen/close.md) — close open pen PRs.
- [++"nave pen prune"++](pen/prune.md) — remove pens that have run but reference deleted remotes.
- [++"nave pen rewrite"++](pen/rewrite.md) — apply declarative rewrites.

In the meantime: use [++"nave pen exec"++](pen/exec.md)
for arbitrary per-repo commands, and drive PRs manually with `gh`.

## Subcommand pages

- [`create`](pen/create.md)
- [`list`](pen/list.md)
- [`show`](pen/show.md)
- [`status`](pen/status.md)
- [`sync`](pen/sync.md)
- [`clean`](pen/clean.md)
- [`revert`](pen/revert.md)
- [`reinit`](pen/reinit.md)
- [`exec`](pen/exec.md)
- [`rm`](pen/rm.md)
- [`rewrite`](pen/rewrite.md)

### Apply-mode verbs

Land a re-derived, authorized proposal onto a real repo via a branch + PR — see the
[Pens](../../concepts/pens.md) concept page. Nave is the sole clone mutator for apply-mode
landings; orchestration and policy live in the caller (e.g. `hiivmind-pulse-gh`'s apply
driver). Sequence: `capabilities` (handshake) → `branch` → `commit` → `push` → `reset`
(cleanup, terminal or on failure).

- [`capabilities`](pen/capabilities.md)
- [`branch`](pen/branch.md)
- [`commit`](pen/commit.md)
- [`push`](pen/push.md)
- [`reset`](pen/reset.md)

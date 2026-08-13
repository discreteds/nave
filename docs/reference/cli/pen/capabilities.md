# ++"nave pen capabilities"++

Report the apply-verb protocol version and the verbs this build of Nave supports.

## Usage

```bash
--8<-- "docs/_snippets/cli/pen/capabilities.txt"
```

## Output

```json
{
  "protocol_version": 1,
  "verbs": ["branch", "commit", "push", "reset"],
  "adapter_state": "ok"
}
```

No pen argument — this is a protocol-level probe, not tied to any pen instance.

## Use cases

- **Handshake before an apply run.** A caller (e.g. the `hiivmind-pulse-gh` apply driver)
  checks `protocol_version` and `verbs` before running `pen branch`/`commit`/`push`/`reset`
  against a pen. A stale Nave binary simply doesn't have the `capabilities` subcommand at all
  — `clap`'s own "unrecognized subcommand" failure is the fail-closed signal for "Nave is too
  old," not something this command needs to simulate.
- **Version pinning in CI.** Confirm the installed Nave build speaks the expected apply
  protocol before wiring automation against it.

## See also

The apply-verb family: [`branch`](branch.md), [`commit`](commit.md), [`push`](push.md),
[`reset`](reset.md).

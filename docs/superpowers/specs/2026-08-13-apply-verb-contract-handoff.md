# Apply-Verb Contract Handoff — for `hiivmind-pulse-gh` Task 1

**Status:** shipped (this repo, `feature/apply-verbs-plan` → `master`, PR #2).
**Read this before implementing `hiivmind-pulse-gh`'s Task 1** (`nave_adapter.py`'s
`pen_capabilities`/`pen_branch`/`pen_commit`/`pen_push`/`pen_reset`). The pulse-gh plan's own
Authoritative Interfaces table (`docs/superpowers/plans/2026-07-30-apply-mode-pulse-wiring.md`
Task 1) left several wire-level details unspecified — this file is the actual, implemented,
tested contract those Python adapters must decode. Where the two disagree, **this file wins**;
the pulse-gh plan's table should be amended to cite it rather than re-deriving the contract.

Source: `docs/superpowers/plans/2026-08-13-apply-verbs.md` (this repo's implementation plan,
executed task-by-task, TDD, 145 tests green). `nave_apply` crate (`crates/nave_apply/src/lib.rs`)
is the canonical source of truth — this document transcribes it for cross-repo convenience;
if the two ever drift, the crate is authoritative.

## The five coordination points

1. **Per-repo `state` enum values are closed sets, kebab-case on the wire.** The pulse-gh
   adapter's `_validate_apply_result` (or equivalent) must check membership against these exact
   strings — an unrecognized value is a validation error, never silently accepted:
   - `branch`: `ok`, `stale-base`, `exists`, `missing-ref`, `not-a-commit`, `unknown-repo`
   - `commit`: `ok`, `nothing-to-commit`, `dirty-outside-bounds`, `invariant-violated`,
     `missing-clone`, `no-apply-state`, `unknown-repo`
   - `push`: `ok`, `missing-branch`, `diverged`, `push-rejected`, `no-apply-state`, `unknown-repo`
   - `reset`: `ok`, `remote-cas-mismatch`, `missing-branch`, `unknown-repo`
   - envelope-level `adapter_state`: `ok`, `error` (snake_case, not kebab-case — different field)
2. **`pen branch`'s `apply_ref` is a single envelope-level field, not per repo.** One branch
   provisioning call names one apply branch across every repo in the request. Build the request
   as `{"protocol_version":1,"apply_ref":"pulse/apply/{id}","repos":[{repo,base_ref,expected_base_sha}, ...]}`.
3. **`pen commit`'s request carries only `{repo, paths}` per repo.** Neither `expected_base_sha`
   (Nave checks this itself against a server-side sidecar `pen branch` wrote) nor `message` (a
   separate CLI/function argument, `-m <message>`) belongs in the request body. Python's
   `pen_commit(runner, name, request, message)` signature already has the right shape — just
   don't try to also stuff `message` into `request`.
4. **An `"error"` envelope's `repos` array is always empty; the failure reason is the top-level
   `reason` field.** Every `*Result` type gained a top-level `reason: Option<String>` alongside
   the existing per-repo `reason` fields. The adapter's envelope-error path should read
   `result["reason"]`, never scan `result["repos"]` for a sentinel/placeholder entry.
5. **`pen status --json` gains `clone_path: Option<String>` per repo** (`null` when the clone
   directory doesn't exist) — the only change to an existing verb. `pen_status`'s Python decode
   shape is otherwise unchanged (still the array-rooted `Vec<RepoState>`, normalized to
   `{"repos": [...]}` by the adapter as before).

## Wire types, verbatim (from `crates/nave_apply/src/lib.rs`)

```rust
pub const PROTOCOL_VERSION: u32 = 1;
pub const APPLY_VERBS: &[&str] = &["branch", "commit", "push", "reset"];

pub enum AdapterState { Ok, Error }   // snake_case

pub struct CapabilitiesResult { protocol_version: u32, verbs: Vec<String>, adapter_state: AdapterState, reason: Option<String> }

pub struct BranchEnvelope { protocol_version: u32, apply_ref: String, repos: Vec<BranchRepoRequest> }
pub struct BranchRepoRequest { repo: String, base_ref: String, expected_base_sha: String }
pub enum BranchState { Ok, StaleBase, Exists, MissingRef, NotACommit, UnknownRepo }   // kebab-case
pub struct BranchRepoResult { repo: String, base_ref: String, expected_base_sha: String, observed_base_sha: String, apply_ref: String, state: BranchState, reason: Option<String> }
pub struct BranchResult { protocol_version: u32, adapter_state: AdapterState, reason: Option<String>, repos: Vec<BranchRepoResult> }

pub struct CommitEnvelope { protocol_version: u32, repos: Vec<CommitRepoRequest> }   // no message field
pub struct CommitRepoRequest { repo: String, paths: Vec<String> }
pub enum CommitState { Ok, NothingToCommit, DirtyOutsideBounds, InvariantViolated, MissingClone, NoApplyState, UnknownRepo }
pub struct CommitRepoResult { repo: String, local_commit_sha: Option<String>, state: CommitState, reason: Option<String> }
pub struct CommitResult { protocol_version: u32, adapter_state: AdapterState, reason: Option<String>, repos: Vec<CommitRepoResult> }

pub struct PushEnvelope { protocol_version: u32, repos: Vec<PushRepoRequest> }
pub struct PushRepoRequest { repo: String }
pub enum PushState { Ok, MissingBranch, Diverged, PushRejected, NoApplyState, UnknownRepo }
pub struct PushRepoResult { repo: String, remote: Option<String>, remote_ref: Option<String>, remote_sha: Option<String>, upstream: Option<String>, local_commit_sha: Option<String>, state: PushState, reason: Option<String> }
pub struct PushResult { protocol_version: u32, adapter_state: AdapterState, reason: Option<String>, repos: Vec<PushRepoResult> }

pub struct ResetEnvelope { protocol_version: u32, repos: Vec<ResetRepoRequest> }
pub struct ResetRepoRequest { repo: String, expected_pushed_sha: Option<String> }   // omit or null when never pushed
pub enum ResetState { Ok, RemoteCasMismatch, MissingBranch, UnknownRepo }
pub struct ResetRepoResult { repo: String, local_reset: bool, remote_deleted: bool, state: ResetState, reason: Option<String> }
pub struct ResetResult { protocol_version: u32, adapter_state: AdapterState, reason: Option<String>, repos: Vec<ResetRepoResult> }
```

Every request type is `deny_unknown_fields` — an unexpected key in the JSON is a parse error,
surfaced by the CLI as an `"error"` envelope (see below), never silently ignored. Every
optional result field (`reason`, `local_commit_sha`, etc.) is omitted from the JSON when `None`
(`skip_serializing_if`), not emitted as `null`.

## CLI invocation shapes (argv-builder reference)

```
nave pen capabilities [--json]
nave pen branch <NAME> --request <FILE> [--json]
nave pen commit <NAME> <BRANCH> --request <FILE> -m/--message <MESSAGE> [--json]
nave pen push <NAME> <BRANCH> --request <FILE> [--json]
nave pen reset <NAME> <BRANCH> --request <FILE> [--json]
```

- `<NAME>` is the pen name (positional, always first).
- `<BRANCH>` is the apply branch name — a positional, **not** part of the request body — for
  `commit`/`push`/`reset` (matches the pulse-gh interface table's own
  `pen_commit(runner, name, request, message)` / `pen_push(runner, name, branch, request)` /
  `pen_reset(runner, name, branch, request)` signatures).
- `--request <FILE>` always names a file path — the JSON body is **never** passed as an inline
  shell argument (mirrors the existing `nave materialize --request` convention).
- `capabilities` takes no pen name — it's a protocol-level probe, not tied to a pen instance.
- **Malformed request JSON, an unsupported `protocol_version`, or a failed wire-shape validation
  (bad ref name, bad SHA, bad repo identity, bad bound path) is caught by the CLI and printed as
  a valid `*Result` envelope with `adapter_state: "error"` and a top-level `reason` — the
  process still exits non-zero, but stdout is always valid JSON when `--json` was passed** (the
  existing `nave materialize` contract: "a defined relationship between process exit status and
  valid JSON").

## Capability handshake

Before any mutation, run `nave pen capabilities --json` and check:
- the command exists at all (a stale/pre-apply-verb Nave binary has no `pen capabilities`
  subcommand — `clap`'s "unrecognized subcommand" nonzero exit **is** the fail-closed signal,
  not something `capabilities` itself needs to simulate);
- `protocol_version == 1`;
- `verbs` is a superset of `["branch", "commit", "push", "reset"]`.

## What Nave does NOT do (still pulse-gh's responsibility)

- **No re-derivation, authorization, or single-repo enforcement.** Nave verbs take whatever
  `expected_base_sha`/`apply_ref`/bound `paths` they're given and enforce them mechanically —
  they don't know what a "proposal" is. `hiivmind-pulse-gh`'s Task 2 (`apply_rederive.py`) and
  the driver's pre-mutation gating (single-repo check, authorization) are unaffected by anything
  in this repo.
- **No phase journal, no lease/fencing.** Crash-resumability and same-machine mutual exclusion
  are the driver's job (`hiivmind-pulse-gh` Tasks 6–8). Nave's own `apply_state` sidecar is a
  much smaller thing: just "what base SHA and origin URL did `branch` verify," scoped to a
  single apply branch, replaced wholesale by the next `branch` call and cleared by `reset` — it
  is not a crash-recovery journal and makes no ordering guarantees across separate verb
  invocations beyond what each verb's own precondition checks re-verify.
- **No F8 finalizer, no PR opening, no merge detection.** Entirely out of scope here.

## Cross-check before implementing pulse-gh's Task 1

Verified against the real `discreteds/nave` binary in this repo, not just this document:
`nave pen <verb> --help` for each of the five verbs reflects the flag/positional shapes above
(`docs/_snippets/cli/pen/{capabilities,branch,commit,push,reset}.txt`), and
`crates/nave/tests/pen_apply.rs::full_apply_lifecycle_lands_on_real_clone_and_cleans_up` proves
the full `capabilities → branch → commit → push → reset` sequence against a real local git
remote, asserting on-disk branch state before/after each step — not just parsed JSON. Re-run
`cargo nextest run --no-fail-fast` in this repo if anything here looks stale relative to a newer
commit on `master`.

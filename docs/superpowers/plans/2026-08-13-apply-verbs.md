# Nave Apply-Verb Extensions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add five `nave pen` verbs — `capabilities`, `branch`, `commit`, `push`, `reset` — implementing the apply-mode git-mutation contract that `hiivmind-pulse-gh`'s Path-A apply driver consumes. Nave becomes the **sole** clone mutator for apply-mode landings; `hiivmind-pulse-gh` keeps orchestration, policy, and read-only verification only.

**Architecture:** A new contract-only crate `nave_apply` (mirrors the existing `nave_materialize` crate: versioned envelopes, `deny_unknown_fields`, deterministic serialization, no git/network code) defines the wire types every verb speaks, plus request-shape validation (protocol version, repo/ref/sha/path syntax). A new `nave_pen::apply_ops` module (mirrors the existing `ops.rs` mutation idiom) implements the git plumbing against those types, using three new internal helpers: `git_util` (captured-stdout git invocation — `ops.rs`'s existing helpers are fire-and-forget and don't return output), `apply_state` (a per-pen, per-apply-branch sidecar recording the provisioned base SHA and origin URL across the separate process invocations each verb runs as — mirrors the existing `rewrite_state.rs` sidecar pattern in this same crate, written atomically via temp-file-then-rename), and repo-identity resolution against the loaded `Pen` (never trusting a caller-supplied `owner/name` string as a filesystem path without checking it names a repo the pen actually has). CLI wiring extends `PenAction` in `crates/nave/src/commands/pen.rs` with five new subcommands, following the existing `--request <file> --json` idiom from `nave materialize`.

**Tech Stack:** Rust workspace (edition 2024), `clap` derive, `serde`/`serde_json`, `tokio` (process), `anyhow`, `thiserror`, `tracing`, `cargo nextest`. One new dev-only dependency: `tempfile` (for real-git-repo fixtures — this plan's tests are the first in the codebase to exercise git mutation against real repos rather than mocked HTTP).

**Source spec:** `hiivmind-pulse-gh`'s `docs/superpowers/specs/2026-07-30-apply-mode-production-wiring-design.md` §3 (already adversarially reviewed twice, Codex `gpt-5.6-sol`) is this plan's design spec — read it first. `hiivmind-pulse-gh`'s `docs/superpowers/plans/2026-07-30-apply-mode-pulse-wiring.md` Task 1 is the **Python-side contract this plan must satisfy**: its Authoritative Interfaces table specifies the five `nave_adapter.py` function signatures (`pen_capabilities`, `pen_branch`, `pen_commit`, `pen_push`, `pen_reset`) that decode exactly the JSON this plan's verbs emit. Where that table leaves a detail unspecified, **this plan is the first to implement and therefore the first to fix that detail** — see "Coordination with hiivmind-pulse-gh" below.

**Verified against the current repo (no drift as of 2026-08-13):** `crates/nave/src/commands/pen.rs`'s `PenAction` enum has `Create/List/Show/Status/Sync/Clean/Revert/Reinit/Exec/Rm/Rewrite` only — none of these five verbs exist yet. `crates/nave_pen/src/create.rs`'s `clone_and_branch` confirms every real pen clone is checked out onto `pen.branch` immediately after cloning (`git checkout -b <pen_branch> <default_branch>`, falling back to a plain checkout if the branch already exists) — this plan's fixtures (Task 1) replicate that exactly, since an earlier draft of this plan got it wrong and left the fixture on the default branch, which would have made Task 9's own reset tests fail against a real pen.

**Revision note:** this is the second draft. The first draft went through one round of adversarial design review (Codex `gpt-5.6-sol`, plan-only, six blocking + five major + one minor finding — full report at `history://` for this session if needed). All findings are folded into this draft: a crash-inconsistent/collision-prone sidecar, insufficient post-exec invariants relative to the source spec's own four-part list (this draft was only checking two of the four), a TOCTOU race in the reset verb's remote-branch CAS, a fixture bug that would fail Task 9's own tests, envelope-level error handling that violated the plan's own "exact coverage" constraint, a `commit` request/CLI field conflict, an inconsistent `apply_ref` placement (per-repo in the request but singular everywhere else), unvalidated repo/ref/sha/path inputs reaching git commands unchecked, and several missing test cases. Nothing here has been implemented — this is still a plan-only artifact.

## Global Constraints

- **No new production runtime dependencies.** Reuse `anyhow`, `thiserror`, `serde`, `serde_json`, `tokio`, `clap`, `tracing`, `time`, already in `[workspace.dependencies]`. `tempfile` is dev-only.
- **Every verb request and result carries `protocol_version` (currently `1`, `nave_apply::PROTOCOL_VERSION`).** A request whose `protocol_version` doesn't match is an envelope-level `error`, never silently accepted.
- **Envelope `adapter_state` is `"ok"` or `"error"` only** — `"ok"` means the command ran and every requested repo got a determinate per-repo outcome (even if that outcome is itself a controlled failure, e.g. `stale-base`). `"error"` is reserved for envelope-level failures the caller cannot attribute to a single repo (bad `protocol_version`, malformed/empty request, duplicate/invalid repo identity, invalid ref/sha/path syntax) — an `"error"` envelope carries a top-level `reason` and an **empty** `repos` array, never a fabricated per-repo entry (a fabricated entry would itself violate exact-coverage).
- **Per-repo `state` is a closed `serde` enum, never a free string** — an invalid value is a deserialization error on the Python side by construction.
- **Exact request-repo coverage applies only to `"ok"` envelopes.** An `"ok"` envelope's `repos` contains exactly one entry per repo named in the request — no extra, no missing, no duplicate. Coverage is validated before any git command runs.
- **Malformed input never becomes an opaque process crash.** A request file that fails to parse as JSON, or that names an unsupported `protocol_version`, or that fails wire-shape validation, is caught by the CLI layer and printed as a valid `"error"` envelope on stdout before the process exits non-zero — the caller always gets JSON back, per the existing `materialize` command's own contract ("a defined relationship between process exit status and valid JSON").
- **Fail closed.** A missing clone directory, an unknown repo (not part of the loaded pen), a failed git command, or a violated invariant is a per-repo failure state — never a fabricated `"ok"`.
- **Caller input never reaches a git command unvalidated.** Every `repo` identity is resolved against the loaded `Pen`'s own repo list (never trusted as a raw filesystem-path component); every `base_ref`/`apply_ref` passes `nave_apply::validate_ref_name`; every `expected_base_sha`/`expected_pushed_sha` passes `nave_apply::validate_hex_sha`; every bound `path` passes `nave_apply::validate_bound_path` (rejects empty, absolute, `..`-containing, and `.git`-prefixed entries).
- **This plan does not delete `hiivmind-pulse-gh`'s raw-git trio** (`provision_apply_branch`/`commit_apply_clones`/`push_apply_clones` in `nave_adapter.py`). That deletion is pulse-gh's own Task 1, gated on this plan shipping — out of scope here.
- **Threat model, stated explicitly (source spec §3 item 5's actual concern):** the invariant checks in `commit_bound` defend against an **ecosystem command or its lifecycle hooks behaving unexpectedly** (e.g. a package manager's `postinstall` switching branches or committing) — not against a deliberately adversarial local process, which already has filesystem access to the sidecar and the git remote and cannot be fully contained by a same-machine, same-user CLI. Within that scope: the apply branch must still be checked out, `HEAD` must still equal the provisioned base, the `origin` remote URL must be unchanged, and `git commit` itself runs with hooks disabled (`-c core.hooksPath=/dev/null`) so nothing planted by the ecosystem command fires during Nave's own commit. Full adversarial hardening (submodule/gitlink handling, symlink-escape analysis beyond path-string validation) is out of scope for v1 and not silently assumed — this note is the boundary, not a placeholder for future work implied elsewhere.
- **`just test` (`cargo nextest run --no-fail-fast`) and `just lint` (`cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --all -- --check`) pass before every task's commit.**
- Land on `master` — confirmed as this fork's PR-integration branch (PR #1 merged to `master`; `origin/develop` here tracks the upstream project's own line and carries unrelated history, e.g. `#18` "Pen rewriting", `#19`/`#20` pre-commit bumps — not this fork's work).

## Authoritative Interfaces (single source of truth — every task and test uses these verbatim)

```rust
// nave_apply (Task 3) — contract-only crate, no git/network, mirrors nave_materialize
pub const PROTOCOL_VERSION: u32 = 1;
pub const APPLY_VERBS: &[&str] = &["branch", "commit", "push", "reset"];

pub enum AdapterState { Ok, Error }   // #[serde(rename_all = "snake_case")]

pub struct CapabilitiesResult { pub protocol_version: u32, pub verbs: Vec<String>, pub adapter_state: AdapterState, pub reason: Option<String> }

// `apply_ref` lives on the ENVELOPE (one ref per branch-provisioning call), not per repo —
// a mixed-ref request within one call is nonsensical and was a contract bug in draft 1.
pub struct BranchEnvelope { pub protocol_version: u32, pub apply_ref: String, pub repos: Vec<BranchRepoRequest> }
pub struct BranchRepoRequest { pub repo: String, pub base_ref: String, pub expected_base_sha: String }
pub enum BranchState { Ok, StaleBase, Exists, MissingRef, NotACommit, UnknownRepo }   // #[serde(rename_all = "kebab-case")]
// `apply_ref` is echoed per repo in the RESULT (even though the request carries it once) so
// every result row is self-describing for logging/debugging — matches the echoed-field
// cross-check pattern the pulse-gh Python adapter already applies to `expected_base_sha`.
pub struct BranchRepoResult { pub repo: String, pub base_ref: String, pub expected_base_sha: String, pub observed_base_sha: String, pub apply_ref: String, pub state: BranchState, pub reason: Option<String> }
pub struct BranchResult { pub protocol_version: u32, pub adapter_state: AdapterState, pub reason: Option<String>, pub repos: Vec<BranchRepoResult> }

// `message` is NOT part of the request envelope — it's a separate CLI/function argument,
// matching the pulse-gh interface table's own `pen_commit(runner, name, request, message)`
// signature (draft 1 incorrectly folded it into the envelope and then had the CLI overwrite
// it, which left the envelope's own field permanently dead and the E2E test's request file
// failing to deserialize).
pub struct CommitEnvelope { pub protocol_version: u32, pub repos: Vec<CommitRepoRequest> }
pub struct CommitRepoRequest { pub repo: String, pub paths: Vec<String> }
pub enum CommitState { Ok, NothingToCommit, DirtyOutsideBounds, InvariantViolated, MissingClone, NoApplyState, UnknownRepo }
pub struct CommitRepoResult { pub repo: String, pub local_commit_sha: Option<String>, pub state: CommitState, pub reason: Option<String> }
pub struct CommitResult { pub protocol_version: u32, pub adapter_state: AdapterState, pub reason: Option<String>, pub repos: Vec<CommitRepoResult> }

pub struct PushEnvelope { pub protocol_version: u32, pub repos: Vec<PushRepoRequest> }
pub struct PushRepoRequest { pub repo: String }
pub enum PushState { Ok, MissingBranch, Diverged, PushRejected, NoApplyState, UnknownRepo }
pub struct PushRepoResult { pub repo: String, pub remote: Option<String>, pub remote_ref: Option<String>, pub remote_sha: Option<String>, pub upstream: Option<String>, pub local_commit_sha: Option<String>, pub state: PushState, pub reason: Option<String> }
pub struct PushResult { pub protocol_version: u32, pub adapter_state: AdapterState, pub reason: Option<String>, pub repos: Vec<PushRepoResult> }

pub struct ResetEnvelope { pub protocol_version: u32, pub repos: Vec<ResetRepoRequest> }
pub struct ResetRepoRequest { pub repo: String, pub expected_pushed_sha: Option<String> }  // None = never pushed; skip remote CAS delete
pub enum ResetState { Ok, RemoteCasMismatch, MissingBranch, UnknownRepo }
pub struct ResetRepoResult { pub repo: String, pub local_reset: bool, pub remote_deleted: bool, pub state: ResetState, pub reason: Option<String> }
pub struct ResetResult { pub protocol_version: u32, pub adapter_state: AdapterState, pub reason: Option<String>, pub repos: Vec<ResetRepoResult> }

#[derive(thiserror::Error, Debug)]
pub enum ValidationError {
    ProtocolVersionMismatch(u32), EmptyRepos, DuplicateRepo(String), InvalidRepoIdentity(String),
    InvalidRefName(String), InvalidSha(String), InvalidPath(String),
}
pub fn validate_envelope_repos(protocol_version: u32, repos: &[String]) -> Result<(), ValidationError>;
pub fn validate_ref_name(name: &str) -> Result<(), ValidationError>;   // no `..`, no leading/trailing `/`, no `//`, no control chars, non-empty
pub fn validate_hex_sha(sha: &str) -> Result<(), ValidationError>;    // exactly 40 lowercase-or-mixed hex chars
pub fn validate_bound_path(path: &str) -> Result<(), ValidationError>; // non-empty, relative, no `..` segment, not under `.git`

// nave_pen::apply_state (Task 4, pub(crate) — internal sidecar, not part of the wire contract)
pub(crate) struct ApplyState { pub repos: BTreeMap<String, ApplyRepoState> }         // key = "owner/name"
pub(crate) struct ApplyRepoState { pub base_ref: String, pub expected_base_sha: String, pub expected_origin_url: String, pub local_commit_sha: Option<String> }
pub(crate) fn read_apply_state(pen_root: &Path, pen_name: &str, apply_ref: &str) -> Result<ApplyState>;
// Written atomically (temp file + rename) and called once per repo transition, not batched
// after a whole loop — so a crash between two repos in the same request leaves the
// already-processed repos durably recorded.
pub(crate) fn write_apply_state(pen_root: &Path, pen_name: &str, apply_ref: &str, state: &ApplyState) -> Result<()>;
pub(crate) fn clear_apply_state(pen_root: &Path, pen_name: &str, apply_ref: &str) -> Result<()>;

// nave_pen::git_util (Task 4, pub(crate))
pub(crate) async fn git_output(dir: &Path, args: &[&str]) -> Result<String>;   // captured, trimmed stdout; bail with stderr on nonzero exit
pub(crate) async fn git_status(dir: &Path, args: &[&str]) -> Result<()>;       // side-effect only, discards stdout
pub(crate) async fn git_ok(dir: &Path, args: &[&str]) -> Result<bool>;         // existence probes — nonzero exit is `false`, not an error

// nave_pen::apply_ops (Tasks 5-9) — the five verb implementations
pub fn capabilities() -> CapabilitiesResult;
pub async fn provision_branch(pen_root: &Path, pen: &Pen, request: &BranchEnvelope) -> Result<BranchResult>;
pub async fn commit_bound(pen_root: &Path, pen: &Pen, apply_ref: &str, message: &str, request: &CommitEnvelope) -> Result<CommitResult>;
pub async fn push_branch(pen_root: &Path, pen: &Pen, apply_ref: &str, request: &PushEnvelope) -> Result<PushResult>;
pub async fn reset_branch(pen_root: &Path, pen: &Pen, apply_ref: &str, request: &ResetEnvelope) -> Result<ResetResult>;
// internal — repo identity is NEVER trusted as a raw path component; always resolved first
pub(crate) fn resolve_repo<'a>(pen: &'a Pen, repo_id: &str) -> Option<&'a nave_pen::storage::PenRepo>;

// nave_test_support (Task 1, dev-dependency only)
pub struct PenFixture { pub origin: TempDir, pub pen_root: TempDir, pub pen: Pen, pub base_sha: String }
pub async fn init_pen_fixture(pen_name: &str, owner: &str, repo: &str, default_branch: &str) -> PenFixture;
```

### Coordination with `hiivmind-pulse-gh` (read before implementing Task 1's Python adapters)

This plan fixes details the pulse-gh plan's Task 1 table left open, because this side implements first:

1. **Per-repo `state` enums** (listed above per verb, including the `unknown-repo` value every verb gained during review) — the pulse-gh `nave_adapter.py` `_validate_apply_result` helper's "state enum invalid" test must check membership against these exact kebab-case strings.
2. **`pen_branch`'s `apply_ref` is a single envelope-level field, not per repo.** The Python adapter's argv/request builder should write `{"protocol_version":1,"apply_ref":"pulse/apply/{id}","repos":[{repo,base_ref,expected_base_sha}, ...]}` — one ref, many repos.
3. **`pen_commit`'s request carries only `{repo, paths}`.** Neither `expected_base_sha` (checked Nave-side against the `apply_state` sidecar) nor `message` (a separate CLI/function argument, `-m <message>`) belongs in the request body.
4. **An `"error"` envelope's `repos` array is always empty; the failure reason is `reason` at the top level**, not synthesized into a fake per-repo row. The Python adapter's envelope-level error path should read `result["reason"]`, not scan `result["repos"]` for a sentinel entry.
5. **`pen_status --json` gains `clone_path: Option<String>` per repo** (Task 2) — `null` when the clone directory doesn't exist.

When `hiivmind-pulse-gh`'s Task 1 lands, its Authoritative Interfaces table should be amended to cite this file rather than leaving these details unspecified — flagged for that plan's own review, not actioned here (cross-repo doc edits stay in their own repo).

---

### Task 1: `nave_test_support` — real-git fixture crate

**Files:** Create `crates/nave_test_support/Cargo.toml`, `crates/nave_test_support/src/lib.rs`; Modify `Cargo.toml` (workspace: add `nave_test_support` to `[workspace.dependencies]`, add `tempfile = {version = "3"}`); Modify `crates/nave_pen/Cargo.toml` (dev-dependency: `tempfile`, `nave_test_support`); Modify `crates/nave/Cargo.toml` (dev-dependency: `nave_test_support`).

**Interfaces:** `init_pen_fixture` from the table. Builds a **bare** repo in one tempdir (acts as `origin`), seeds one commit on `default_branch`, records its SHA, clones it into a second tempdir laid out exactly like `pen_repo_clone_dir` expects, and — matching `create_pen`'s own `clone_and_branch` exactly — checks out `pen.branch` (`checkout -b <pen.branch> <default_branch>`) so the fixture reflects what a real pen clone always looks like. Persists `pen.toml` via `nave_pen::storage::write_pen` so `load_pen` can find it. Sets a deterministic local git identity so `git commit` doesn't depend on the host's global config.

- [ ] **Step 1: Write failing test**

```rust
// crates/nave_test_support/src/lib.rs — the crate's own smoke check
#[tokio::test]
async fn fixture_clone_is_checked_out_on_pen_branch() {
    let fx = init_pen_fixture("apply-fixture", "acme", "docs", "develop").await;
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "apply-fixture", "acme", "docs");
    let out = tokio::process::Command::new("git")
        .arg("-C").arg(&dir).args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output().await.unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), fx.pen.branch);
    let loaded = nave_pen::load_pen(fx.pen_root.path(), "apply-fixture").unwrap();
    assert_eq!(loaded.name, "apply-fixture");
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p nave_test_support` fails: crate/module don't exist.
- [ ] **Step 3: Implement**

```toml
# crates/nave_test_support/Cargo.toml
[dependencies]
nave_pen = {workspace = true}
tokio = {workspace = true}
time = {workspace = true}
tempfile = {workspace = true}
anyhow = {workspace = true}

[lints]
workspace = true

[package]
name = "nave_test_support"
version = "0.0.1"
description = "Real-git fixtures for pen apply-verb tests"
authors.workspace = true
edition.workspace = true
homepage.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true
```

```rust
// crates/nave_test_support/src/lib.rs
use std::path::PathBuf;

use nave_pen::{Pen, PenFilter, PenRepo};
use tempfile::TempDir;
use time::OffsetDateTime;
use tokio::process::Command;

pub struct PenFixture {
    pub origin: TempDir,
    pub pen_root: TempDir,
    pub pen: Pen,
    pub base_sha: String,
}

async fn git(dir: &std::path::Path, args: &[&str]) {
    let status = Command::new("git").arg("-C").arg(dir).args(args).status().await.unwrap();
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

pub async fn init_pen_fixture(pen_name: &str, owner: &str, repo: &str, default_branch: &str) -> PenFixture {
    let origin = TempDir::new().unwrap();
    git(origin.path(), &["init", "--bare", "-b", default_branch]).await;

    let seed = TempDir::new().unwrap();
    git(seed.path(), &["clone", origin.path().to_str().unwrap(), "."]).await;
    std::fs::write(seed.path().join("README.md"), "seed\n").unwrap();
    git(seed.path(), &["add", "README.md"]).await;
    git(seed.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "seed"]).await;
    git(seed.path(), &["push", "origin", default_branch]).await;
    let sha_out = Command::new("git").arg("-C").arg(seed.path()).args(["rev-parse", "HEAD"]).output().await.unwrap();
    let base_sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();

    let pen_root = TempDir::new().unwrap();
    let clone_dir: PathBuf = nave_pen::pen_repo_clone_dir(pen_root.path(), pen_name, owner, repo);
    git(pen_root.path(), &["clone", origin.path().to_str().unwrap(), clone_dir.to_str().unwrap()]).await;
    let pen_branch = format!("nave/{pen_name}");
    // Mirror create_pen's clone_and_branch exactly: checkout -b <pen_branch> <default_branch>,
    // falling back to a plain checkout if the branch already exists (it never will here, but
    // the fallback keeps the fixture's behavior identical to production, not just similar).
    let checkout = Command::new("git").arg("-C").arg(&clone_dir)
        .args(["checkout", "-b", &pen_branch, default_branch]).status().await.unwrap();
    if !checkout.success() {
        git(&clone_dir, &["checkout", &pen_branch]).await;
    }
    git(&clone_dir, &["config", "user.email", "t@t"]).await;
    git(&clone_dir, &["config", "user.name", "t"]).await;

    let pen = Pen {
        name: pen_name.to_string(),
        created_at: OffsetDateTime::now_utc(),
        branch: pen_branch,
        filter: PenFilter::default(),
        repos: vec![PenRepo {
            owner: owner.to_string(),
            name: repo.to_string(),
            default_branch: default_branch.to_string(),
            clone_url: format!("file://{}", origin.path().display()),
            synced_at: OffsetDateTime::now_utc(),
        }],
        ops: vec![],
    };
    nave_pen::storage::write_pen(pen_root.path(), &pen).unwrap();

    PenFixture { origin, pen_root, pen, base_sha }
}
```

- [ ] **Step 4: Run, verify pass** — `cargo test -p nave_test_support`.
- [ ] **Step 5: Commit** — `test: real-git pen fixture crate for apply-verb tests`.

---

### Task 2: `pen status --json` gains `clone_path`

**Files:** Modify `crates/nave_pen/src/state.rs`; Create `crates/nave_pen/tests/status_clone_path.rs`.

**Interfaces:** `RepoState` gains `pub clone_path: Option<String>` (`None` when the clone directory doesn't exist). This is spec §3 item 6 (the read-path capability `pen_clone_reader` needs) — small and independent of the other four tasks, land it first since it touches no new module.

- [ ] **Step 1: Write failing test**

```rust
// crates/nave_pen/tests/status_clone_path.rs
use nave_pen::compute_repo_state;

#[tokio::test]
async fn clone_path_present_when_repo_cloned() {
    let fx = nave_test_support::init_pen_fixture("status-fx", "acme", "docs", "main").await;
    let cache = tempfile::TempDir::new().unwrap();
    let state = compute_repo_state(fx.pen_root.path(), cache.path(), &fx.pen, &fx.pen.repos[0]).await.unwrap();
    let expected = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), &fx.pen.name, "acme", "docs");
    assert_eq!(state.clone_path.as_deref(), Some(expected.to_str().unwrap()));
}

#[tokio::test]
async fn clone_path_absent_when_repo_missing() {
    let pen_root = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let pen = nave_pen::Pen {
        name: "no-clone".into(), created_at: time::OffsetDateTime::now_utc(),
        branch: "nave/no-clone".into(), filter: nave_pen::PenFilter::default(),
        repos: vec![nave_pen::PenRepo {
            owner: "acme".into(), name: "docs".into(), default_branch: "main".into(),
            clone_url: "file:///dev/null".into(), synced_at: time::OffsetDateTime::now_utc(),
        }],
        ops: vec![],
    };
    let state = compute_repo_state(pen_root.path(), cache.path(), &pen, &pen.repos[0]).await.unwrap();
    assert_eq!(state.clone_path, None);
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p nave_pen --test status_clone_path` fails: no `clone_path` field.
- [ ] **Step 3: Implement.** In `state.rs`, add the field to `RepoState` and both construction sites in `compute_repo_state`:

```rust
// struct RepoState — add:
pub clone_path: Option<String>,

// missing-dir branch — add:
clone_path: None,

// present branch — add just before the final Ok(RepoState { … }):
let clone_path = Some(dir.to_string_lossy().into_owned());
// then in the struct literal: clone_path,
```

Add `tempfile`, `nave_test_support`, and a `tokio` dev-feature set to `crates/nave_pen/Cargo.toml`'s `[dev-dependencies]`.

- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit** — `feat: expose clone_path in pen status --json`.

---

### Task 3: `nave_apply` contract crate

**Files:** Create `crates/nave_apply/Cargo.toml`, `crates/nave_apply/src/lib.rs`, `crates/nave_apply/tests/contract.rs`; Modify root `Cargo.toml` (`[workspace.dependencies]`: add `nave_apply = {path = "crates/nave_apply"}`).

**Interfaces:** every type and validation function in the Authoritative Interfaces table's `nave_apply` block. Mirrors `nave_materialize`'s conventions: `#[serde(deny_unknown_fields)]` on every request type, a custom `Serialize` impl on envelope types that sorts `repos` lexically by `repo` before emitting, `#[serde(rename_all = "kebab-case")]` on state enums, `#[serde(rename_all = "snake_case")]` on `AdapterState`, `#[serde(skip_serializing_if = "Option::is_none")]` on every optional field.

- [ ] **Step 1: Write failing tests**

```rust
// crates/nave_apply/tests/contract.rs
use nave_apply::{
    AdapterState, BranchEnvelope, BranchRepoRequest, BranchState, PROTOCOL_VERSION,
    ValidationError, validate_bound_path, validate_envelope_repos, validate_hex_sha, validate_ref_name,
};

fn branch_req(repo: &str) -> BranchRepoRequest {
    BranchRepoRequest { repo: repo.into(), base_ref: "develop".into(), expected_base_sha: "a".repeat(40) }
}

#[test]
fn protocol_version_mismatch_is_rejected() {
    let err = validate_envelope_repos(2, &["acme/docs".into()]).unwrap_err();
    assert!(matches!(err, ValidationError::ProtocolVersionMismatch(2)));
}

#[test]
fn empty_repos_is_rejected() {
    assert!(matches!(validate_envelope_repos(PROTOCOL_VERSION, &[]), Err(ValidationError::EmptyRepos)));
}

#[test]
fn duplicate_repo_is_rejected() {
    let repos = vec!["acme/docs".to_string(), "acme/docs".to_string()];
    assert!(matches!(validate_envelope_repos(PROTOCOL_VERSION, &repos), Err(ValidationError::DuplicateRepo(_))));
}

#[test]
fn non_owner_name_identity_is_rejected() {
    let repos = vec!["docs".to_string()];
    assert!(matches!(validate_envelope_repos(PROTOCOL_VERSION, &repos), Err(ValidationError::InvalidRepoIdentity(_))));
}

#[test]
fn ref_name_with_parent_traversal_is_rejected() {
    assert!(validate_ref_name("pulse/../etc").is_err());
}

#[test]
fn ref_name_with_leading_slash_is_rejected() {
    assert!(validate_ref_name("/pulse/apply/p1").is_err());
}

#[test]
fn valid_ref_name_passes() {
    assert!(validate_ref_name("pulse/apply/p1").is_ok());
}

#[test]
fn sha_must_be_exactly_40_hex_chars() {
    assert!(validate_hex_sha(&"a".repeat(40)).is_ok());
    assert!(validate_hex_sha(&"a".repeat(39)).is_err());
    assert!(validate_hex_sha("not-hex-not-hex-not-hex-not-hex-not-hex").is_err());
}

#[test]
fn bound_path_rejects_traversal_and_absolute_and_git_dir() {
    assert!(validate_bound_path("../secret").is_err());
    assert!(validate_bound_path("/etc/passwd").is_err());
    assert!(validate_bound_path(".git/config").is_err());
    assert!(validate_bound_path("package-lock.json").is_ok());
}

#[test]
fn unknown_keys_in_branch_request_are_rejected() {
    let raw = r#"{"protocol_version":1,"apply_ref":"y","repos":[{"repo":"a/b","base_ref":"main","expected_base_sha":"x","extra":true}]}"#;
    assert!(serde_json::from_str::<BranchEnvelope>(raw).is_err());
}

#[test]
fn branch_state_serializes_kebab_case() {
    assert_eq!(serde_json::to_string(&BranchState::StaleBase).unwrap(), "\"stale-base\"");
}

#[test]
fn adapter_state_serializes_snake_case() {
    assert_eq!(serde_json::to_string(&AdapterState::Error).unwrap(), "\"error\"");
}

#[test]
fn branch_envelope_serializes_repos_sorted_regardless_of_construction_order() {
    let e1 = BranchEnvelope { protocol_version: 1, apply_ref: "pulse/apply/p1".into(), repos: vec![branch_req("z/z"), branch_req("a/a")] };
    let e2 = BranchEnvelope { protocol_version: 1, apply_ref: "pulse/apply/p1".into(), repos: vec![branch_req("a/a"), branch_req("z/z")] };
    assert_eq!(serde_json::to_string(&e1).unwrap(), serde_json::to_string(&e2).unwrap());
}
```

- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement.**

```toml
# crates/nave_apply/Cargo.toml
[dependencies]
serde = {workspace = true}
serde_json = {workspace = true}
thiserror = {workspace = true}

[lints]
workspace = true

[package]
name = "nave_apply"
version = "0.0.1"
description = "Apply-mode git-mutation verb contract for nave pens"
authors.workspace = true
edition.workspace = true
homepage.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true
```

```rust
// crates/nave_apply/src/lib.rs — validation
use serde::{Deserialize, Serialize, Serializer};

pub const PROTOCOL_VERSION: u32 = 1;
pub const APPLY_VERBS: &[&str] = &["branch", "commit", "push", "reset"];

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("protocol_version {0} is not supported (expected {PROTOCOL_VERSION})")]
    ProtocolVersionMismatch(u32),
    #[error("request must name at least one repo")]
    EmptyRepos,
    #[error("duplicate repo in request: {0}")]
    DuplicateRepo(String),
    #[error("repo identity must be owner/name: {0}")]
    InvalidRepoIdentity(String),
    #[error("invalid ref name: {0}")]
    InvalidRefName(String),
    #[error("invalid sha (must be 40 hex chars): {0}")]
    InvalidSha(String),
    #[error("invalid bound path: {0}")]
    InvalidPath(String),
}

pub fn validate_envelope_repos(protocol_version: u32, repos: &[String]) -> Result<(), ValidationError> {
    if protocol_version != PROTOCOL_VERSION {
        return Err(ValidationError::ProtocolVersionMismatch(protocol_version));
    }
    if repos.is_empty() {
        return Err(ValidationError::EmptyRepos);
    }
    let mut seen = std::collections::HashSet::new();
    for r in repos {
        if !seen.insert(r.as_str()) {
            return Err(ValidationError::DuplicateRepo(r.clone()));
        }
        let parts: Vec<&str> = r.split('/').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(ValidationError::InvalidRepoIdentity(r.clone()));
        }
    }
    Ok(())
}

pub fn validate_ref_name(name: &str) -> Result<(), ValidationError> {
    let bad = name.is_empty()
        || name.starts_with('/') || name.ends_with('/')
        || name.contains("//") || name.contains("..")
        || name.starts_with('-')
        || name.chars().any(|c| c.is_control() || c == ' ' || c == '~' || c == '^' || c == ':' || c == '?' || c == '*' || c == '[');
    if bad { return Err(ValidationError::InvalidRefName(name.to_string())); }
    Ok(())
}

pub fn validate_hex_sha(sha: &str) -> Result<(), ValidationError> {
    if sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(ValidationError::InvalidSha(sha.to_string()))
    }
}

pub fn validate_bound_path(path: &str) -> Result<(), ValidationError> {
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.starts_with('-')
        || path.split('/').any(|seg| seg == "..")
        || path == ".git" || path.starts_with(".git/");
    if bad { return Err(ValidationError::InvalidPath(path.to_string())); }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterState { Ok, Error }

#[derive(Debug, Clone, Serialize)]
pub struct CapabilitiesResult {
    pub protocol_version: u32,
    pub verbs: Vec<String>,
    pub adapter_state: AdapterState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchRepoRequest {
    pub repo: String,
    pub base_ref: String,
    pub expected_base_sha: String,
}

impl Serialize for BranchRepoRequest {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("BranchRepoRequest", 3)?;
        st.serialize_field("repo", &self.repo)?;
        st.serialize_field("base_ref", &self.base_ref)?;
        st.serialize_field("expected_base_sha", &self.expected_base_sha)?;
        st.end()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchEnvelope {
    pub protocol_version: u32,
    pub apply_ref: String,
    pub repos: Vec<BranchRepoRequest>,
}

impl Serialize for BranchEnvelope {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut repos = self.repos.clone();
        repos.sort_by(|a, b| a.repo.cmp(&b.repo));
        let mut st = s.serialize_struct("BranchEnvelope", 3)?;
        st.serialize_field("protocol_version", &self.protocol_version)?;
        st.serialize_field("apply_ref", &self.apply_ref)?;
        st.serialize_field("repos", &repos)?;
        st.end()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BranchState { Ok, StaleBase, Exists, MissingRef, NotACommit, UnknownRepo }

#[derive(Debug, Clone, Serialize)]
pub struct BranchRepoResult {
    pub repo: String,
    pub base_ref: String,
    pub expected_base_sha: String,
    pub observed_base_sha: String,
    pub apply_ref: String,
    pub state: BranchState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BranchResult {
    pub protocol_version: u32,
    pub adapter_state: AdapterState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub repos: Vec<BranchRepoResult>,
}
```

Repeat the `RepoRequest`/`Envelope`/`State`/`RepoResult`/`Result` quintet for `Commit`, `Push`, `Reset` per the Authoritative Interfaces table's field lists (`CommitEnvelope` has no `apply_ref`/`message` field — those are function/CLI arguments; `ResetRepoRequest.expected_pushed_sha` is `Option<String>` — `#[serde(default)]` on deserialize since a repo that was never pushed omits it), each with the same `deny_unknown_fields` request / sorted-`Serialize` envelope / kebab-case state / `skip_serializing_if` optional-field pattern shown above for `Branch`, and each `*Result` type carries the top-level `reason: Option<String>` field.

- [ ] **Step 4: Run, verify pass** (`cargo test -p nave_apply`).
- [ ] **Step 5: Commit** — `feat: nave_apply contract crate (capabilities/branch/commit/push/reset)`.

---

### Task 4: `git_util` + `apply_state` shared modules

**Files:** Create `crates/nave_pen/src/git_util.rs`, `crates/nave_pen/src/apply_state.rs`; Modify `crates/nave_pen/src/lib.rs` (add `mod git_util; mod apply_state;` — both stay private to the crate, no `pub use`); Modify `crates/nave_pen/Cargo.toml` (`toml` is already a dependency).

**Interfaces:** `git_output`/`git_status`/`git_ok` and the `ApplyState` sidecar from the table. The sidecar exists because `provision_branch`, `commit_bound`, `push_branch`, `reset_branch` run as **separate CLI process invocations** — nothing survives in memory between them. Stored at `<pen_dir>/apply/<apply_ref>/state.toml` — `apply_ref` is used directly as nested path *components* (not a single filename with `/` collapsed to `__`, which draft 1 got wrong: `pulse/a__b/c` and `pulse/a/b__c` would have collided). This is safe because every `apply_ref` reaching this function has already passed `nave_apply::validate_ref_name` (no `..`, no leading/trailing/doubled `/`) before any caller can get here — Task 6 enforces that at the entry point of every verb. Writes are atomic: write to a same-directory temp file, then `rename` (POSIX rename is atomic on the same filesystem), so a crash mid-write never leaves a truncated/corrupt TOML file for the next invocation to choke on.

- [ ] **Step 1: Write failing tests** (inline `#[cfg(test)]`, since both modules are `pub(crate)` and unreachable from `crates/nave_pen/tests/*.rs`)

```rust
// crates/nave_pen/src/apply_state.rs — bottom of file
#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ApplyRepoState {
        ApplyRepoState { base_ref: "develop".into(), expected_base_sha: "a".repeat(40), expected_origin_url: "file:///origin".into(), local_commit_sha: None }
    }

    #[test]
    fn round_trips_through_disk() {
        let root = tempfile::TempDir::new().unwrap();
        let mut state = ApplyState::default();
        state.repos.insert("acme/docs".into(), sample());
        write_apply_state(root.path(), "pen1", "pulse/apply/p1", &state).unwrap();
        let read = read_apply_state(root.path(), "pen1", "pulse/apply/p1").unwrap();
        assert_eq!(read.repos["acme/docs"].base_ref, "develop");
    }

    #[test]
    fn missing_state_reads_as_empty() {
        let root = tempfile::TempDir::new().unwrap();
        let state = read_apply_state(root.path(), "pen1", "pulse/apply/never-provisioned").unwrap();
        assert!(state.repos.is_empty());
    }

    #[test]
    fn clear_removes_the_file() {
        let root = tempfile::TempDir::new().unwrap();
        write_apply_state(root.path(), "pen1", "pulse/apply/p1", &ApplyState::default()).unwrap();
        clear_apply_state(root.path(), "pen1", "pulse/apply/p1").unwrap();
        assert!(read_apply_state(root.path(), "pen1", "pulse/apply/p1").unwrap().repos.is_empty());
    }

    #[test]
    fn distinct_refs_sharing_a_slash_boundary_do_not_collide() {
        let root = tempfile::TempDir::new().unwrap();
        let mut a = ApplyState::default();
        a.repos.insert("x/y".into(), sample());
        write_apply_state(root.path(), "pen1", "pulse/a__b/c", &a).unwrap();
        let mut b = ApplyState::default();
        b.repos.insert("p/q".into(), sample());
        write_apply_state(root.path(), "pen1", "pulse/a/b__c", &b).unwrap();
        assert_eq!(read_apply_state(root.path(), "pen1", "pulse/a__b/c").unwrap().repos.len(), 1);
        assert_eq!(read_apply_state(root.path(), "pen1", "pulse/a/b__c").unwrap().repos.len(), 1);
        assert!(read_apply_state(root.path(), "pen1", "pulse/a__b/c").unwrap().repos.contains_key("x/y"));
        assert!(read_apply_state(root.path(), "pen1", "pulse/a/b__c").unwrap().repos.contains_key("p/q"));
    }
}
```

```rust
// crates/nave_pen/src/git_util.rs — bottom of file
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn git_output_captures_trimmed_stdout() {
        let fx = nave_test_support::init_pen_fixture("git-util-fx", "acme", "docs", "main").await;
        let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "git-util-fx", "acme", "docs");
        let sha = git_output(&dir, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(sha, fx.base_sha);
    }

    #[tokio::test]
    async fn git_output_bails_on_nonzero_exit() {
        let fx = nave_test_support::init_pen_fixture("git-util-fx2", "acme", "docs", "main").await;
        let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "git-util-fx2", "acme", "docs");
        assert!(git_output(&dir, &["rev-parse", "refs/heads/does-not-exist"]).await.is_err());
    }

    #[tokio::test]
    async fn git_ok_is_false_not_error_on_missing_ref() {
        let fx = nave_test_support::init_pen_fixture("git-util-fx3", "acme", "docs", "main").await;
        let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "git-util-fx3", "acme", "docs");
        let ok = git_ok(&dir, &["rev-parse", "--verify", "--quiet", "refs/heads/nope"]).await.unwrap();
        assert!(!ok);
    }
}
```

- [ ] **Step 2: Run, verify fail** — modules don't exist yet.
- [ ] **Step 3: Implement.**

```rust
// crates/nave_pen/src/git_util.rs
//! Shared git-plumbing helpers for apply-verb operations. Distinct from
//! `ops.rs`'s fire-and-forget helpers: every function here returns captured
//! stdout, because the apply verbs report exact SHAs/refs back to the
//! caller as structured JSON — git's own text output is the only place
//! that data exists.

use std::path::Path;

use anyhow::{Context, Result, bail};
use tokio::process::Command;

pub(crate) async fn git_output(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C").arg(dir).args(args)
        .output().await
        .with_context(|| format!("spawning git {args:?} in {}", dir.display()))?;
    if !out.status.success() {
        bail!("git {:?} in {} failed: {}", args, dir.display(), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub(crate) async fn git_status(dir: &Path, args: &[&str]) -> Result<()> {
    git_output(dir, args).await.map(|_| ())
}

pub(crate) async fn git_ok(dir: &Path, args: &[&str]) -> Result<bool> {
    let status = Command::new("git")
        .arg("-C").arg(dir).args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().await
        .with_context(|| format!("spawning git {args:?} in {}", dir.display()))?;
    Ok(status.success())
}
```

```rust
// crates/nave_pen/src/apply_state.rs
//! Per-pen, per-apply-branch sidecar: the base SHA and origin URL
//! `provision_branch` verified, persisted because each verb runs as a
//! separate process. `apply_ref` is used as nested directory components
//! (pre-validated by `nave_apply::validate_ref_name` at every call site —
//! no `..`, no leading/trailing/doubled `/`), never collapsed into one
//! filename, so distinct refs sharing a `/`-boundary can never collide.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::storage::pen_dir;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ApplyState {
    #[serde(default)]
    pub repos: BTreeMap<String, ApplyRepoState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ApplyRepoState {
    pub base_ref: String,
    pub expected_base_sha: String,
    pub expected_origin_url: String,
    #[serde(default)]
    pub local_commit_sha: Option<String>,
}

fn apply_state_path(pen_root: &Path, pen_name: &str, apply_ref: &str) -> PathBuf {
    pen_dir(pen_root, pen_name).join("apply").join(apply_ref).join("state.toml")
}

pub(crate) fn read_apply_state(pen_root: &Path, pen_name: &str, apply_ref: &str) -> Result<ApplyState> {
    let path = apply_state_path(pen_root, pen_name, apply_ref);
    if !path.exists() {
        return Ok(ApplyState::default());
    }
    let raw = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

pub(crate) fn write_apply_state(pen_root: &Path, pen_name: &str, apply_ref: &str, state: &ApplyState) -> Result<()> {
    let path = apply_state_path(pen_root, pen_name, apply_ref);
    let parent = path.parent().expect("apply state path always has a parent");
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".state.toml.tmp.{}", std::process::id()));
    std::fs::write(&tmp, toml::to_string_pretty(state)?).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("renaming into {}", path.display()))
}

pub(crate) fn clear_apply_state(pen_root: &Path, pen_name: &str, apply_ref: &str) -> Result<()> {
    let path = apply_state_path(pen_root, pen_name, apply_ref);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}
```

Add `mod git_util;` and `mod apply_state;` to `crates/nave_pen/src/lib.rs` (no `pub use` — both stay crate-internal). Add `tempfile`, `nave_test_support`, `tokio` (rt-multi-thread, macros features) to `[dev-dependencies]` in `crates/nave_pen/Cargo.toml`.

- [ ] **Step 4: Run, verify pass** (`cargo test -p nave_pen`).
- [ ] **Step 5: Commit** — `feat: git_util + apply_state sidecar for apply verbs`.

---

### Task 5: `nave pen capabilities` verb

**Files:** Create `crates/nave_pen/src/apply_ops.rs`; Modify `crates/nave_pen/src/lib.rs` (`pub mod apply_ops;` + re-export `capabilities`, `provision_branch`, `commit_bound`, `push_branch`, `reset_branch` as they land in Tasks 5-9); Modify `crates/nave/src/commands/pen.rs` (`PenAction::Capabilities`, `PenCapabilitiesArgs`, `run_capabilities`); Modify `crates/nave/Cargo.toml` and `crates/nave_pen/Cargo.toml` (`nave_apply = {workspace = true}`); Modify root `Cargo.toml` `[workspace.dependencies]` (`nave_apply` already added in Task 3).

**Interfaces:** `apply_ops::capabilities() -> CapabilitiesResult` — pure, synchronous, no I/O. A stale Nave binary that predates this verb simply doesn't have the `capabilities` subcommand at all, so the pulse-gh handshake's "missing verb" failure mode is `clap`'s own "unrecognized subcommand" nonzero exit.

- [ ] **Step 1: Write failing test**

```rust
// crates/nave_pen/src/apply_ops.rs — bottom of file (module created in this step)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_reports_protocol_and_verbs() {
        let caps = capabilities();
        assert_eq!(caps.protocol_version, nave_apply::PROTOCOL_VERSION);
        assert_eq!(caps.verbs, nave_apply::APPLY_VERBS.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(matches!(caps.adapter_state, nave_apply::AdapterState::Ok));
    }
}
```

- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement.**

```rust
// crates/nave_pen/src/apply_ops.rs — top of file
//! Apply-mode mutation verbs: branch/commit/push/reset over a Nave pen,
//! plus the stateless capabilities probe. Mirrors `ops.rs`'s mutation
//! idiom; uses `git_util` (captured output) and `apply_state` (the
//! cross-invocation sidecar) instead of `ops.rs`'s fire-and-forget helpers.

use nave_apply::{AdapterState, CapabilitiesResult, PROTOCOL_VERSION, APPLY_VERBS};

pub fn capabilities() -> CapabilitiesResult {
    CapabilitiesResult {
        protocol_version: PROTOCOL_VERSION,
        verbs: APPLY_VERBS.iter().map(|s| s.to_string()).collect(),
        adapter_state: AdapterState::Ok,
        reason: None,
    }
}
```

In `crates/nave/src/commands/pen.rs`: add `Capabilities(PenCapabilitiesArgs)` to `PenAction` (doc comment: `/// Report the apply-verb protocol version and supported verbs.`), add:

```rust
#[derive(Debug, Args, Default)]
pub(crate) struct PenCapabilitiesArgs {
    /// Emit JSON instead of a human summary.
    #[arg(long)]
    pub json: bool,
}
```

and in `run`'s match: `PenAction::Capabilities(a) => run_capabilities(a),`, plus:

```rust
fn run_capabilities(args: PenCapabilitiesArgs) -> Result<()> {
    let caps = nave_pen::apply_ops::capabilities();
    if args.json {
        println!("{}", serde_json::to_string_pretty(&caps)?);
    } else {
        println!("protocol_version={} verbs={}", caps.protocol_version, caps.verbs.join(","));
    }
    Ok(())
}
```

- [ ] **Step 4: Run, verify pass** (`cargo test -p nave_pen` + `cargo run --bin nave -- pen capabilities --json` prints the expected object).
- [ ] **Step 5: Commit** — `feat: nave pen capabilities verb`.

---

### Task 6: `nave pen branch` verb — remote-base CAS provisioning + shared request validation

**Files:** Modify `crates/nave_pen/src/apply_ops.rs`; Modify `crates/nave/src/commands/pen.rs` (`PenAction::Branch`, `PenBranchArgs`, `run_branch`).

**Interfaces:** `apply_ops::provision_branch` from the table, plus `resolve_repo` and the shared per-verb request-validation entry point every later task's `*_bound`/`*_branch` function reuses. Per repo: resolve the repo identity against `pen.repos` (never trust the caller string as a path component), fetch `base_ref` fresh from `origin` (never trust a stale local ref), compare the observed remote SHA to `expected_base_sha` (a mismatch is `stale-base`, a reportable outcome, not a hard error), verify the resolved object is a commit, fail closed if `apply_ref` already exists locally (never blind reuse), then `checkout -B` off the verified SHA and persist the provisioned base **and origin URL** into the `apply_state` sidecar immediately after each repo succeeds (not batched after the whole loop).

- [ ] **Step 1: Write failing tests**

```rust
// crates/nave_pen/src/apply_ops.rs — add to the existing `mod tests`
fn branch_req(fx: &nave_test_support::PenFixture, apply_ref: &str) -> nave_apply::BranchEnvelope {
    nave_apply::BranchEnvelope {
        protocol_version: nave_apply::PROTOCOL_VERSION, apply_ref: apply_ref.into(),
        repos: vec![nave_apply::BranchRepoRequest { repo: "acme/docs".into(), base_ref: "develop".into(), expected_base_sha: fx.base_sha.clone() }],
    }
}

#[tokio::test]
async fn branch_provisions_off_verified_remote_base() {
    let fx = nave_test_support::init_pen_fixture("branch-fx", "acme", "docs", "develop").await;
    let res = provision_branch(fx.pen_root.path(), &fx.pen, &branch_req(&fx, "pulse/apply/p1")).await.unwrap();
    assert!(matches!(res.adapter_state, nave_apply::AdapterState::Ok));
    assert!(matches!(res.repos[0].state, nave_apply::BranchState::Ok));
    assert_eq!(res.repos[0].observed_base_sha, fx.base_sha);

    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "branch-fx", "acme", "docs");
    let branch = crate::git_util::git_output(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await.unwrap();
    assert_eq!(branch, "pulse/apply/p1");
}

#[tokio::test]
async fn branch_reports_stale_base_without_creating_branch() {
    let fx = nave_test_support::init_pen_fixture("branch-fx2", "acme", "docs", "develop").await;
    let mut req = branch_req(&fx, "pulse/apply/p1");
    req.repos[0].expected_base_sha = "0".repeat(40);
    let res = provision_branch(fx.pen_root.path(), &fx.pen, &req).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::BranchState::StaleBase));
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "branch-fx2", "acme", "docs");
    let branch = crate::git_util::git_output(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await.unwrap();
    assert_eq!(branch, fx.pen.branch);
}

#[tokio::test]
async fn branch_fails_closed_when_apply_ref_already_exists() {
    let fx = nave_test_support::init_pen_fixture("branch-fx3", "acme", "docs", "develop").await;
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "branch-fx3", "acme", "docs");
    crate::git_util::git_status(&dir, &["checkout", "-B", "pulse/apply/p1"]).await.unwrap();
    crate::git_util::git_status(&dir, &["checkout", &fx.pen.branch]).await.unwrap();
    let res = provision_branch(fx.pen_root.path(), &fx.pen, &branch_req(&fx, "pulse/apply/p1")).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::BranchState::Exists));
}

#[tokio::test]
async fn branch_reports_unknown_repo_not_in_pen() {
    let fx = nave_test_support::init_pen_fixture("branch-fx4", "acme", "docs", "develop").await;
    let mut req = branch_req(&fx, "pulse/apply/p1");
    req.repos[0].repo = "other/repo".into();
    let res = provision_branch(fx.pen_root.path(), &fx.pen, &req).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::BranchState::UnknownRepo));
}

#[tokio::test]
async fn branch_rejects_invalid_ref_name_at_envelope_level() {
    let fx = nave_test_support::init_pen_fixture("branch-fx5", "acme", "docs", "develop").await;
    let mut req = branch_req(&fx, "../escape");
    req.apply_ref = "../escape".into();
    let res = provision_branch(fx.pen_root.path(), &fx.pen, &req).await.unwrap();
    assert!(matches!(res.adapter_state, nave_apply::AdapterState::Error));
    assert!(res.repos.is_empty());
    assert!(res.reason.is_some());
}
```

- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement.**

```rust
// crates/nave_pen/src/apply_ops.rs — append
use std::path::Path;

use anyhow::Result as AResult;

use crate::apply_state::{ApplyRepoState, ApplyState, read_apply_state, write_apply_state, clear_apply_state};
use crate::git_util::{git_ok, git_output, git_status};
use crate::storage::{Pen, PenRepo, pen_repo_clone_dir};

pub(crate) fn resolve_repo<'a>(pen: &'a Pen, repo_id: &str) -> Option<&'a PenRepo> {
    let (owner, name) = repo_id.split_once('/')?;
    pen.repos.iter().find(|r| r.owner == owner && r.name == name)
}

/// Envelope-level error result for a request that fails `nave_apply` validation before any git
/// command runs — empty `repos`, the reason at the top level, never a fabricated per-repo entry.
macro_rules! error_envelope {
    ($result_ty:ident, $err:expr) => {
        return Ok(nave_apply::$result_ty {
            protocol_version: nave_apply::PROTOCOL_VERSION,
            adapter_state: nave_apply::AdapterState::Error,
            reason: Some($err.to_string()),
            repos: vec![],
        })
    };
}

pub async fn provision_branch(
    pen_root: &Path,
    pen: &Pen,
    request: &nave_apply::BranchEnvelope,
) -> AResult<nave_apply::BranchResult> {
    let repo_ids: Vec<String> = request.repos.iter().map(|r| r.repo.clone()).collect();
    if let Err(e) = nave_apply::validate_envelope_repos(request.protocol_version, &repo_ids) {
        error_envelope!(BranchResult, e);
    }
    if let Err(e) = nave_apply::validate_ref_name(&request.apply_ref) {
        error_envelope!(BranchResult, e);
    }
    for r in &request.repos {
        if let Err(e) = nave_apply::validate_ref_name(&r.base_ref) {
            error_envelope!(BranchResult, e);
        }
        if let Err(e) = nave_apply::validate_hex_sha(&r.expected_base_sha) {
            error_envelope!(BranchResult, e);
        }
    }

    let mut results = Vec::with_capacity(request.repos.len());
    for req in &request.repos {
        let result = match resolve_repo(pen, &req.repo) {
            None => nave_apply::BranchRepoResult {
                repo: req.repo.clone(), base_ref: req.base_ref.clone(), expected_base_sha: req.expected_base_sha.clone(),
                observed_base_sha: String::new(), apply_ref: request.apply_ref.clone(),
                state: nave_apply::BranchState::UnknownRepo, reason: Some("repo is not part of this pen".into()),
            },
            Some(pen_repo) => {
                let dir = pen_repo_clone_dir(pen_root, &pen.name, &pen_repo.owner, &pen_repo.name);
                let result = provision_one(&dir, req, &request.apply_ref).await;
                if matches!(result.state, nave_apply::BranchState::Ok) {
                    let origin_url = git_output(&dir, &["remote", "get-url", "origin"]).await.unwrap_or_default();
                    let mut state = read_apply_state(pen_root, &pen.name, &request.apply_ref)?;
                    state.repos.insert(req.repo.clone(), ApplyRepoState {
                        base_ref: req.base_ref.clone(), expected_base_sha: req.expected_base_sha.clone(),
                        expected_origin_url: origin_url, local_commit_sha: None,
                    });
                    write_apply_state(pen_root, &pen.name, &request.apply_ref, &state)?;
                }
                result
            }
        };
        results.push(result);
    }

    Ok(nave_apply::BranchResult { protocol_version: nave_apply::PROTOCOL_VERSION, adapter_state: nave_apply::AdapterState::Ok, reason: None, repos: results })
}

async fn provision_one(dir: &Path, req: &nave_apply::BranchRepoRequest, apply_ref: &str) -> nave_apply::BranchRepoResult {
    let mk = |state, observed: String, reason: Option<&str>| nave_apply::BranchRepoResult {
        repo: req.repo.clone(), base_ref: req.base_ref.clone(), expected_base_sha: req.expected_base_sha.clone(),
        observed_base_sha: observed, apply_ref: apply_ref.to_string(), state, reason: reason.map(str::to_string),
    };
    if !dir.exists() {
        return mk(nave_apply::BranchState::MissingRef, String::new(), Some("clone directory does not exist"));
    }
    if let Err(e) = git_status(dir, &["fetch", "--depth=1", "origin", &req.base_ref]).await {
        return mk(nave_apply::BranchState::MissingRef, String::new(), Some(&e.to_string()));
    }
    let observed = match git_output(dir, &["rev-parse", &format!("origin/{}", req.base_ref)]).await {
        Ok(sha) => sha,
        Err(e) => return mk(nave_apply::BranchState::MissingRef, String::new(), Some(&e.to_string())),
    };
    if git_output(dir, &["cat-file", "-t", &observed]).await.ok().as_deref() != Some("commit") {
        return mk(nave_apply::BranchState::NotACommit, observed, Some("resolved object is not a commit"));
    }
    if observed != req.expected_base_sha {
        return mk(nave_apply::BranchState::StaleBase, observed, Some("observed base sha does not match expected"));
    }
    if git_ok(dir, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{apply_ref}")]).await.unwrap_or(false) {
        return mk(nave_apply::BranchState::Exists, observed, Some("apply branch already exists"));
    }
    if let Err(e) = git_status(dir, &["checkout", "-B", apply_ref, &observed]).await {
        return mk(nave_apply::BranchState::NotACommit, observed, Some(&e.to_string()));
    }
    mk(nave_apply::BranchState::Ok, observed, None)
}
```

CLI wiring in `crates/nave/src/commands/pen.rs`, following the `materialize.rs` `--request FILE` idiom (never inline JSON on the command line) and catching parse failures as a valid `"error"` envelope rather than an opaque process error:

```rust
#[derive(Debug, Args)]
pub(crate) struct PenBranchArgs {
    pub name: String,
    #[arg(long)]
    pub request: std::path::PathBuf,
    #[arg(long)]
    pub json: bool,
}

async fn run_branch(args: PenBranchArgs) -> Result<()> {
    let cfg = load_default()?;
    let root = resolve_pen_root(&cfg.pen)?;
    let pen = load_pen(&root, &args.name)?;
    let raw = std::fs::read_to_string(&args.request).context("reading request file")?;
    let request: nave_apply::BranchEnvelope = match serde_json::from_str(&raw) {
        Ok(r) => r,
        Err(e) => {
            let result = nave_apply::BranchResult { protocol_version: nave_apply::PROTOCOL_VERSION, adapter_state: nave_apply::AdapterState::Error, reason: Some(format!("invalid request: {e}")), repos: vec![] };
            if args.json { println!("{}", serde_json::to_string_pretty(&result)?); }
            std::process::exit(1);
        }
    };
    let result = nave_pen::apply_ops::provision_branch(&root, &pen, &request).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    if matches!(result.adapter_state, nave_apply::AdapterState::Error) {
        std::process::exit(1);
    }
    Ok(())
}
```

Add `Branch(PenBranchArgs)` to `PenAction`, `PenAction::Branch(a) => run_branch(a).await,` to `run`, and `pub use apply_ops::provision_branch;` to `nave_pen`'s `lib.rs`. This same "catch parse errors, emit a valid error envelope, exit non-zero" shape is reused verbatim by Tasks 7-9's CLI wiring — not re-derived per task.

- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit** — `feat: nave pen branch verb (remote-base CAS provisioning)`.

---

### Task 7: `nave pen commit` verb — bounded staging + the source spec's full post-exec invariant set

**Files:** Modify `crates/nave_pen/src/apply_ops.rs`; Modify `crates/nave/src/commands/pen.rs` (`PenAction::Commit`, `PenCommitArgs`, `run_commit`).

**Interfaces:** `apply_ops::commit_bound` from the table. Loads the `apply_state` sidecar `provision_branch` wrote; per repo, before staging, verifies **all four** invariants the source spec's item 5 actually lists (draft 1 only checked two of these): the apply branch is still checked out, `HEAD` still equals the provisioned base, **the `origin` remote URL is unchanged**, and every dirty path (parsed correctly, including renames, from `git status --porcelain`) is within the requested `paths` — any dirty path outside them fails the commit closed (`dirty-outside-bounds`), never `add -A`. Stages only the requested paths (each individually validated by `nave_apply::validate_bound_path` before ever reaching a git command), commits with hooks disabled (`-c core.hooksPath=/dev/null`, so nothing an ecosystem command planted in `.git/hooks` fires during Nave's own commit), then re-verifies the resulting diff touched only bound paths before accepting the result — and updates the sidecar with the resulting SHA immediately (not batched).

- [ ] **Step 1: Write failing tests**

```rust
fn commit_req(paths: &[&str]) -> nave_apply::CommitEnvelope {
    nave_apply::CommitEnvelope { protocol_version: nave_apply::PROTOCOL_VERSION, repos: vec![nave_apply::CommitRepoRequest { repo: "acme/docs".into(), paths: paths.iter().map(|s| s.to_string()).collect() }] }
}

async fn provisioned(name: &str, apply_ref: &str) -> nave_test_support::PenFixture {
    let fx = nave_test_support::init_pen_fixture(name, "acme", "docs", "develop").await;
    provision_branch(fx.pen_root.path(), &fx.pen, &branch_req(&fx, apply_ref)).await.unwrap();
    fx
}

#[tokio::test]
async fn commit_stages_only_bound_paths() {
    let fx = provisioned("commit-fx", "pulse/apply/c1").await;
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "commit-fx", "acme", "docs");
    std::fs::write(dir.join("lockfile.json"), "{}").unwrap();
    let res = commit_bound(fx.pen_root.path(), &fx.pen, "pulse/apply/c1", "bump lockfile", &commit_req(&["lockfile.json"])).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::CommitState::Ok));
    assert!(res.repos[0].local_commit_sha.is_some());
}

#[tokio::test]
async fn commit_fails_closed_on_dirty_path_outside_bounds() {
    let fx = provisioned("commit-fx2", "pulse/apply/c2").await;
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "commit-fx2", "acme", "docs");
    std::fs::write(dir.join("lockfile.json"), "{}").unwrap();
    std::fs::write(dir.join("unexpected.txt"), "surprise").unwrap();
    let res = commit_bound(fx.pen_root.path(), &fx.pen, "pulse/apply/c2", "m", &commit_req(&["lockfile.json"])).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::CommitState::DirtyOutsideBounds));
    let status = crate::git_util::git_output(&dir, &["status", "--porcelain"]).await.unwrap();
    assert!(!status.is_empty(), "nothing should have been committed");
}

#[tokio::test]
async fn commit_fails_closed_when_origin_remote_changed() {
    let fx = provisioned("commit-fx3", "pulse/apply/c3").await;
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "commit-fx3", "acme", "docs");
    crate::git_util::git_status(&dir, &["remote", "set-url", "origin", "file:///somewhere-else"]).await.unwrap();
    std::fs::write(dir.join("lockfile.json"), "{}").unwrap();
    let res = commit_bound(fx.pen_root.path(), &fx.pen, "pulse/apply/c3", "m", &commit_req(&["lockfile.json"])).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::CommitState::InvariantViolated));
    assert!(res.repos[0].reason.as_deref().unwrap_or_default().contains("origin"));
}

#[tokio::test]
async fn commit_fails_closed_when_no_apply_state_recorded() {
    let fx = nave_test_support::init_pen_fixture("commit-fx4", "acme", "docs", "develop").await;
    let res = commit_bound(fx.pen_root.path(), &fx.pen, "pulse/apply/never-provisioned", "m", &commit_req(&["x"])).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::CommitState::NoApplyState));
}

#[tokio::test]
async fn commit_rejects_invalid_bound_path_at_envelope_level() {
    let fx = provisioned("commit-fx5", "pulse/apply/c5").await;
    let res = commit_bound(fx.pen_root.path(), &fx.pen, "pulse/apply/c5", "m", &commit_req(&["../escape"])).await.unwrap();
    assert!(matches!(res.adapter_state, nave_apply::AdapterState::Error));
    assert!(res.repos.is_empty());
}
```

- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement.**

```rust
// crates/nave_pen/src/apply_ops.rs — append
pub async fn commit_bound(
    pen_root: &Path,
    pen: &Pen,
    apply_ref: &str,
    message: &str,
    request: &nave_apply::CommitEnvelope,
) -> AResult<nave_apply::CommitResult> {
    let repo_ids: Vec<String> = request.repos.iter().map(|r| r.repo.clone()).collect();
    if let Err(e) = nave_apply::validate_envelope_repos(request.protocol_version, &repo_ids) {
        error_envelope!(CommitResult, e);
    }
    if let Err(e) = nave_apply::validate_ref_name(apply_ref) {
        error_envelope!(CommitResult, e);
    }
    for r in &request.repos {
        for p in &r.paths {
            if let Err(e) = nave_apply::validate_bound_path(p) {
                error_envelope!(CommitResult, e);
            }
        }
    }

    let mut state = read_apply_state(pen_root, &pen.name, apply_ref)?;
    let mut results = Vec::with_capacity(request.repos.len());
    for req in &request.repos {
        let Some(pen_repo) = resolve_repo(pen, &req.repo) else {
            results.push(nave_apply::CommitRepoResult { repo: req.repo.clone(), local_commit_sha: None, state: nave_apply::CommitState::UnknownRepo, reason: Some("repo is not part of this pen".into()) });
            continue;
        };
        let dir = pen_repo_clone_dir(pen_root, &pen.name, &pen_repo.owner, &pen_repo.name);
        let Some(repo_state) = state.repos.get(&req.repo).cloned() else {
            results.push(nave_apply::CommitRepoResult { repo: req.repo.clone(), local_commit_sha: None, state: nave_apply::CommitState::NoApplyState, reason: Some("no provisioned base recorded for this apply branch".into()) });
            continue;
        };
        let result = commit_one(&dir, req, apply_ref, &repo_state, message).await;
        if let (nave_apply::CommitState::Ok, Some(sha)) = (&result.state, &result.local_commit_sha) {
            state.repos.get_mut(&req.repo).unwrap().local_commit_sha = Some(sha.clone());
            write_apply_state(pen_root, &pen.name, apply_ref, &state)?;
        }
        results.push(result);
    }

    Ok(nave_apply::CommitResult { protocol_version: nave_apply::PROTOCOL_VERSION, adapter_state: nave_apply::AdapterState::Ok, reason: None, repos: results })
}

fn dirty_paths_from_porcelain(porcelain: &str) -> Vec<String> {
    porcelain.lines().filter(|l| l.len() > 3).flat_map(|l| {
        let rest = l[3..].trim_matches('"');
        if let Some((old, new)) = rest.split_once(" -> ") {
            vec![old.to_string(), new.to_string()]
        } else {
            vec![rest.to_string()]
        }
    }).collect()
}

async fn commit_one(dir: &Path, req: &nave_apply::CommitRepoRequest, apply_ref: &str, repo_state: &ApplyRepoState, message: &str) -> nave_apply::CommitRepoResult {
    let mk = |state, sha: Option<String>, reason: Option<&str>| nave_apply::CommitRepoResult { repo: req.repo.clone(), local_commit_sha: sha, state, reason: reason.map(str::to_string) };
    if !dir.exists() {
        return mk(nave_apply::CommitState::MissingClone, None, Some("clone directory does not exist"));
    }

    // The source spec's four post-exec invariants, in full — draft 1 only checked the first two.
    let branch = match git_output(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await { Ok(b) => b, Err(e) => return mk(nave_apply::CommitState::InvariantViolated, None, Some(&e.to_string())) };
    if branch != apply_ref {
        return mk(nave_apply::CommitState::InvariantViolated, None, Some("checked-out branch changed since provisioning"));
    }
    let head = match git_output(dir, &["rev-parse", "HEAD"]).await { Ok(h) => h, Err(e) => return mk(nave_apply::CommitState::InvariantViolated, None, Some(&e.to_string())) };
    if head != repo_state.expected_base_sha {
        return mk(nave_apply::CommitState::InvariantViolated, None, Some("HEAD moved since provisioning — unexpected commit during exec"));
    }
    let origin_url = match git_output(dir, &["remote", "get-url", "origin"]).await { Ok(u) => u, Err(e) => return mk(nave_apply::CommitState::InvariantViolated, None, Some(&e.to_string())) };
    if origin_url != repo_state.expected_origin_url {
        return mk(nave_apply::CommitState::InvariantViolated, None, Some("origin remote url changed since provisioning"));
    }
    let porcelain = match git_output(dir, &["status", "--porcelain"]).await { Ok(p) => p, Err(e) => return mk(nave_apply::CommitState::InvariantViolated, None, Some(&e.to_string())) };
    let dirty = dirty_paths_from_porcelain(&porcelain);
    let bound: std::collections::HashSet<&str> = req.paths.iter().map(String::as_str).collect();
    if let Some(extra) = dirty.iter().find(|p| !bound.contains(p.as_str())) {
        return mk(nave_apply::CommitState::DirtyOutsideBounds, None, Some(&format!("{extra} is dirty but not in bound_paths")));
    }
    if dirty.is_empty() {
        return mk(nave_apply::CommitState::NothingToCommit, None, None);
    }

    for p in &req.paths {
        if git_status(dir, &["add", "--", p]).await.is_err() {
            return mk(nave_apply::CommitState::InvariantViolated, None, Some(&format!("failed to stage {p}")));
        }
    }
    // Hooks disabled for Nave's own commit: nothing an ecosystem command planted in
    // `.git/hooks` (pre-commit, commit-msg, etc.) fires during this call.
    if let Err(e) = git_status(dir, &["-c", "core.hooksPath=/dev/null", "commit", "-m", message]).await {
        return mk(nave_apply::CommitState::InvariantViolated, None, Some(&e.to_string()));
    }
    let sha = match git_output(dir, &["rev-parse", "HEAD"]).await { Ok(s) => s, Err(e) => return mk(nave_apply::CommitState::InvariantViolated, None, Some(&e.to_string())) };

    // Post-commit bound check: the committed tree must touch only the requested paths, even
    // accounting for anything a hook could have done despite hooksPath being disabled above.
    let changed = match git_output(dir, &["diff", "--name-only", &repo_state.expected_base_sha, &sha]).await { Ok(c) => c, Err(e) => return mk(nave_apply::CommitState::InvariantViolated, Some(sha), Some(&e.to_string())) };
    if let Some(extra) = changed.lines().find(|p| !bound.contains(*p)) {
        return mk(nave_apply::CommitState::InvariantViolated, Some(sha), Some(&format!("committed tree touched {extra}, outside bound_paths")));
    }

    mk(nave_apply::CommitState::Ok, Some(sha), None)
}
```

CLI wiring mirrors Task 6's `PenBranchArgs`/`run_branch` shape (including the "catch parse failure → error envelope → exit 1" wrapper) with an added positional `branch` (the `apply_ref`) and `-m/--message`:

```rust
#[derive(Debug, Args)]
pub(crate) struct PenCommitArgs {
    pub name: String,
    pub branch: String,
    #[arg(long)]
    pub request: std::path::PathBuf,
    #[arg(short = 'm', long)]
    pub message: String,
    #[arg(long)]
    pub json: bool,
}

async fn run_commit(args: PenCommitArgs) -> Result<()> {
    let cfg = load_default()?;
    let root = resolve_pen_root(&cfg.pen)?;
    let pen = load_pen(&root, &args.name)?;
    let raw = std::fs::read_to_string(&args.request).context("reading request file")?;
    let request: nave_apply::CommitEnvelope = match serde_json::from_str(&raw) {
        Ok(r) => r,
        Err(e) => {
            let result = nave_apply::CommitResult { protocol_version: nave_apply::PROTOCOL_VERSION, adapter_state: nave_apply::AdapterState::Error, reason: Some(format!("invalid request: {e}")), repos: vec![] };
            if args.json { println!("{}", serde_json::to_string_pretty(&result)?); }
            std::process::exit(1);
        }
    };
    let result = nave_pen::apply_ops::commit_bound(&root, &pen, &args.branch, &args.message, &request).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    if matches!(result.adapter_state, nave_apply::AdapterState::Error) {
        std::process::exit(1);
    }
    Ok(())
}
```

- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit** — `feat: nave pen commit verb (bounded staging, full post-exec invariant set)`.

---

### Task 8: `nave pen push` verb — structured results, exact coverage, verified evidence

**Files:** Modify `crates/nave_pen/src/apply_ops.rs`; Modify `crates/nave/src/commands/pen.rs` (`PenAction::Push`, `PenPushArgs`, `run_push`).

**Interfaces:** `apply_ops::push_branch` from the table. Reads the sidecar's recorded `local_commit_sha` and `expected_origin_url` per repo; before pushing, verifies `refs/heads/<apply_ref>` (not just whatever `HEAD` happens to be — a repo could be on a different branch at the same commit) still equals the recorded SHA, and that `origin`'s URL is unchanged since provisioning; pushes with an explicit fully-qualified refspec (`refs/heads/<apply_ref>:refs/heads/<apply_ref>`, never a bare branch-name shorthand that could resolve ambiguously); reads back `remote_sha` via `git rev-parse origin/<apply_ref>` after the push and treats a failure to read any of `remote`/`remote_ref`/`remote_sha`/`upstream` as a push-evidence failure, not a silently-`Ok` result with holes in it.

- [ ] **Step 1: Write failing tests**

```rust
async fn provisioned_and_committed(name: &str, apply_ref: &str) -> (nave_test_support::PenFixture, String) {
    let fx = provisioned(name, apply_ref).await;
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), name, "acme", "docs");
    std::fs::write(dir.join("lockfile.json"), "{}").unwrap();
    let commit_res = commit_bound(fx.pen_root.path(), &fx.pen, apply_ref, "m", &commit_req(&["lockfile.json"])).await.unwrap();
    (fx, commit_res.repos[0].local_commit_sha.clone().unwrap())
}

fn push_req() -> nave_apply::PushEnvelope {
    nave_apply::PushEnvelope { protocol_version: nave_apply::PROTOCOL_VERSION, repos: vec![nave_apply::PushRepoRequest { repo: "acme/docs".into() }] }
}

#[tokio::test]
async fn push_reports_remote_sha_matching_local_commit() {
    let (fx, local_sha) = provisioned_and_committed("push-fx", "pulse/apply/pu1").await;
    let res = push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/pu1", &push_req()).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::PushState::Ok));
    assert_eq!(res.repos[0].remote_sha.as_deref(), Some(local_sha.as_str()));
    assert_eq!(res.repos[0].remote_ref.as_deref(), Some("pulse/apply/pu1"));
}

#[tokio::test]
async fn push_is_idempotent_on_identical_history() {
    let (fx, _) = provisioned_and_committed("push-fx2", "pulse/apply/pu2").await;
    push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/pu2", &push_req()).await.unwrap();
    let second = push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/pu2", &push_req()).await.unwrap();
    assert!(matches!(second.repos[0].state, nave_apply::PushState::Ok));
}

#[tokio::test]
async fn push_fails_closed_without_a_prior_commit() {
    let fx = provisioned("push-fx3", "pulse/apply/pu3").await;
    let res = push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/pu3", &push_req()).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::PushState::NoApplyState));
}

#[tokio::test]
async fn push_fails_closed_when_origin_remote_changed_since_commit() {
    let (fx, _) = provisioned_and_committed("push-fx4", "pulse/apply/pu4").await;
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "push-fx4", "acme", "docs");
    crate::git_util::git_status(&dir, &["remote", "set-url", "origin", "file:///elsewhere"]).await.unwrap();
    let res = push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/pu4", &push_req()).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::PushState::PushRejected));
}
```

- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement.**

```rust
// crates/nave_pen/src/apply_ops.rs — append
pub async fn push_branch(
    pen_root: &Path,
    pen: &Pen,
    apply_ref: &str,
    request: &nave_apply::PushEnvelope,
) -> AResult<nave_apply::PushResult> {
    let repo_ids: Vec<String> = request.repos.iter().map(|r| r.repo.clone()).collect();
    if let Err(e) = nave_apply::validate_envelope_repos(request.protocol_version, &repo_ids) {
        error_envelope!(PushResult, e);
    }
    if let Err(e) = nave_apply::validate_ref_name(apply_ref) {
        error_envelope!(PushResult, e);
    }
    let state = read_apply_state(pen_root, &pen.name, apply_ref)?;
    let mut results = Vec::with_capacity(request.repos.len());
    for req in &request.repos {
        let Some(pen_repo) = resolve_repo(pen, &req.repo) else {
            results.push(nave_apply::PushRepoResult { repo: req.repo.clone(), remote: None, remote_ref: None, remote_sha: None, upstream: None, local_commit_sha: None, state: nave_apply::PushState::UnknownRepo, reason: Some("repo is not part of this pen".into()) });
            continue;
        };
        let dir = pen_repo_clone_dir(pen_root, &pen.name, &pen_repo.owner, &pen_repo.name);
        results.push(push_one(&dir, &req.repo, apply_ref, state.repos.get(&req.repo).cloned()).await);
    }
    Ok(nave_apply::PushResult { protocol_version: nave_apply::PROTOCOL_VERSION, adapter_state: nave_apply::AdapterState::Ok, reason: None, repos: results })
}

async fn push_one(dir: &Path, repo: &str, apply_ref: &str, repo_state: Option<ApplyRepoState>) -> nave_apply::PushRepoResult {
    let mk = |state, remote, remote_ref, remote_sha, upstream, local_commit_sha, reason: Option<&str>| nave_apply::PushRepoResult {
        repo: repo.to_string(), remote, remote_ref, remote_sha, upstream, local_commit_sha, state, reason: reason.map(str::to_string),
    };
    let Some(repo_state) = repo_state else {
        return mk(nave_apply::PushState::NoApplyState, None, None, None, None, None, Some("no provisioned/committed state recorded for this repo"));
    };
    let Some(expected_local_sha) = repo_state.local_commit_sha.clone() else {
        return mk(nave_apply::PushState::NoApplyState, None, None, None, None, None, Some("no committed local sha recorded for this repo"));
    };
    if !dir.exists() {
        return mk(nave_apply::PushState::MissingBranch, None, None, None, None, None, Some("clone directory does not exist"));
    }
    let branch_sha = match git_output(dir, &["rev-parse", &format!("refs/heads/{apply_ref}")]).await { Ok(s) => s, Err(e) => return mk(nave_apply::PushState::MissingBranch, None, None, None, None, None, Some(&e.to_string())) };
    if branch_sha != expected_local_sha {
        return mk(nave_apply::PushState::Diverged, None, None, None, None, Some(branch_sha), Some("apply branch tip does not match the recorded commit"));
    }
    let origin_url = match git_output(dir, &["remote", "get-url", "origin"]).await { Ok(u) => u, Err(e) => return mk(nave_apply::PushState::PushRejected, None, None, None, None, Some(branch_sha), Some(&e.to_string())) };
    if origin_url != repo_state.expected_origin_url {
        return mk(nave_apply::PushState::PushRejected, None, None, None, None, Some(branch_sha), Some("origin remote url changed since provisioning"));
    }
    if let Err(e) = git_status(dir, &["push", "origin", &format!("refs/heads/{apply_ref}:refs/heads/{apply_ref}")]).await {
        return mk(nave_apply::PushState::PushRejected, None, None, None, None, Some(branch_sha), Some(&e.to_string()));
    }
    let remote = match git_output(dir, &["remote", "get-url", "origin"]).await { Ok(u) => u, Err(_) => return mk(nave_apply::PushState::PushRejected, None, None, None, None, Some(branch_sha), Some("push succeeded but remote url could not be re-read")) };
    let remote_sha = match git_output(dir, &["rev-parse", &format!("origin/{apply_ref}")]).await { Ok(s) => s, Err(_) => return mk(nave_apply::PushState::PushRejected, None, None, None, None, Some(branch_sha), Some("push succeeded but remote sha could not be verified")) };
    let upstream = git_output(dir, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]).await.ok();
    mk(nave_apply::PushState::Ok, Some(remote), Some(apply_ref.to_string()), Some(remote_sha), upstream, Some(branch_sha), None)
}
```

CLI wiring mirrors Task 7's `PenCommitArgs` shape minus `-m/--message` (just `name`, `branch` positional, `--request`, `--json`, same parse-error-to-envelope wrapper).

- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit** — `feat: nave pen push verb (structured results, verified evidence)`.

---

### Task 9: `nave pen reset` verb — atomic CAS-guarded cleanup

**Files:** Modify `crates/nave_pen/src/apply_ops.rs`; Modify `crates/nave/src/commands/pen.rs` (`PenAction::Reset`, `PenResetArgs`, `run_reset`).

**Interfaces:** `apply_ops::reset_branch` from the table. Local cleanup always runs (idempotent): discard any dirty state first (`reset --hard` + `clean -fd`, harmless on an already-clean tree), check out `pen.branch` if currently on the apply branch (guaranteed to exist locally per Task 1's fixture and `create_pen`'s own `clone_and_branch` — this is a correctness fix from the review, not a fixture-only patch: draft 1's fixture never checked out `pen.branch` at all, which is not what real pens look like), then delete the local apply branch — a checkout failure is reported, never silently swallowed. Remote deletion is a **single atomic `git push --force-with-lease=refs/heads/<apply_ref>:<expected_sha> origin :refs/heads/<apply_ref>`** call — not a separate `ls-remote` read followed by an unconditional delete, which draft 1 used and which has a TOCTOU race (verified locally during review: a replacement push between the two calls would have let the unconditional delete remove someone else's branch). `--force-with-lease` performs the SHA comparison and the delete as one atomic server-side operation; a lease rejection (`stale info` in stderr) is reported as `remote-cas-mismatch`, distinct from any other push failure (auth/network), which is reported with its own descriptive reason.

- [ ] **Step 1: Write failing tests**

```rust
fn reset_req(expected_pushed_sha: Option<String>) -> nave_apply::ResetEnvelope {
    nave_apply::ResetEnvelope { protocol_version: nave_apply::PROTOCOL_VERSION, repos: vec![nave_apply::ResetRepoRequest { repo: "acme/docs".into(), expected_pushed_sha }] }
}

#[tokio::test]
async fn reset_deletes_remote_ref_only_on_sha_match() {
    let (fx, local_sha) = provisioned_and_committed("reset-fx", "pulse/apply/r1").await;
    push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/r1", &push_req()).await.unwrap();

    let res = reset_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/r1", &reset_req(Some(local_sha))).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::ResetState::Ok));
    assert!(res.repos[0].local_reset);
    assert!(res.repos[0].remote_deleted);

    let remote_refs = crate::git_util::git_output(fx.origin.path(), &["for-each-ref", "refs/heads/pulse/apply/r1"]).await.unwrap();
    assert!(remote_refs.is_empty());

    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "reset-fx", "acme", "docs");
    let branch = crate::git_util::git_output(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await.unwrap();
    assert_eq!(branch, fx.pen.branch);
}

#[tokio::test]
async fn reset_skips_remote_delete_on_cas_mismatch() {
    let (fx, _) = provisioned_and_committed("reset-fx2", "pulse/apply/r2").await;
    push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/r2", &push_req()).await.unwrap();

    let res = reset_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/r2", &reset_req(Some("f".repeat(40)))).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::ResetState::RemoteCasMismatch));
    assert!(!res.repos[0].remote_deleted);
    let remote_refs = crate::git_util::git_output(fx.origin.path(), &["for-each-ref", "refs/heads/pulse/apply/r2"]).await.unwrap();
    assert!(!remote_refs.is_empty(), "remote branch must survive a CAS mismatch");
}

#[tokio::test]
async fn reset_is_idempotent_when_called_twice() {
    let (fx, local_sha) = provisioned_and_committed("reset-fx3", "pulse/apply/r3").await;
    push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/r3", &push_req()).await.unwrap();
    reset_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/r3", &reset_req(Some(local_sha.clone()))).await.unwrap();
    let second = reset_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/r3", &reset_req(Some(local_sha))).await.unwrap();
    assert!(matches!(second.repos[0].state, nave_apply::ResetState::Ok));
    assert!(second.repos[0].local_reset);
}

#[tokio::test]
async fn reset_handles_never_pushed_repo_without_touching_remote() {
    let fx = provisioned("reset-fx4", "pulse/apply/r4").await;
    let res = reset_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/r4", &reset_req(None)).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::ResetState::Ok));
    assert!(res.repos[0].local_reset);
    assert!(!res.repos[0].remote_deleted);
}
```

- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement.**

```rust
// crates/nave_pen/src/apply_ops.rs — append
pub async fn reset_branch(
    pen_root: &Path,
    pen: &Pen,
    apply_ref: &str,
    request: &nave_apply::ResetEnvelope,
) -> AResult<nave_apply::ResetResult> {
    let repo_ids: Vec<String> = request.repos.iter().map(|r| r.repo.clone()).collect();
    if let Err(e) = nave_apply::validate_envelope_repos(request.protocol_version, &repo_ids) {
        error_envelope!(ResetResult, e);
    }
    if let Err(e) = nave_apply::validate_ref_name(apply_ref) {
        error_envelope!(ResetResult, e);
    }
    for r in &request.repos {
        if let Some(sha) = &r.expected_pushed_sha {
            if let Err(e) = nave_apply::validate_hex_sha(sha) {
                error_envelope!(ResetResult, e);
            }
        }
    }

    let mut results = Vec::with_capacity(request.repos.len());
    for req in &request.repos {
        let result = match resolve_repo(pen, &req.repo) {
            None => nave_apply::ResetRepoResult { repo: req.repo.clone(), local_reset: false, remote_deleted: false, state: nave_apply::ResetState::UnknownRepo, reason: Some("repo is not part of this pen".into()) },
            Some(pen_repo) => {
                let dir = pen_repo_clone_dir(pen_root, &pen.name, &pen_repo.owner, &pen_repo.name);
                reset_one(&dir, &pen.branch, apply_ref, req).await
            }
        };
        results.push(result);
    }
    clear_apply_state(pen_root, &pen.name, apply_ref)?;
    Ok(nave_apply::ResetResult { protocol_version: nave_apply::PROTOCOL_VERSION, adapter_state: nave_apply::AdapterState::Ok, reason: None, repos: results })
}

async fn reset_one(dir: &Path, pen_branch: &str, apply_ref: &str, req: &nave_apply::ResetRepoRequest) -> nave_apply::ResetRepoResult {
    if !dir.exists() {
        return nave_apply::ResetRepoResult { repo: req.repo.clone(), local_reset: false, remote_deleted: false, state: nave_apply::ResetState::MissingBranch, reason: Some("clone directory does not exist".into()) };
    }

    // Local cleanup is always attempted and is idempotent — discard any dirty state first
    // (harmless no-op on a clean tree), move off the apply branch if it's checked out, then
    // delete it. Every step's failure is reported, never silently discarded.
    let _ = git_status(dir, &["reset", "--hard", "HEAD"]).await;
    let _ = git_status(dir, &["clean", "-fd"]).await;
    let mut local_reset = true;
    let mut reason: Option<String> = None;

    let on_apply_ref = git_output(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await.ok().as_deref() == Some(apply_ref);
    if on_apply_ref {
        if let Err(e) = git_status(dir, &["checkout", pen_branch]).await {
            local_reset = false;
            reason = Some(format!("failed to check out {pen_branch} before deleting apply branch: {e}"));
        }
    }
    if reason.is_none() {
        let apply_branch_exists = git_ok(dir, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{apply_ref}")]).await.unwrap_or(false);
        if apply_branch_exists {
            local_reset = git_status(dir, &["branch", "-D", apply_ref]).await.is_ok();
            if !local_reset {
                reason = Some("failed to delete local apply branch".into());
            }
        }
        // else: already gone locally — idempotent no-op, local_reset stays true.
    }

    let mut remote_deleted = false;
    if reason.is_none() {
        if let Some(expected) = &req.expected_pushed_sha {
            // `--force-with-lease` takes an OPTIONAL argument: git only accepts the value
            // glued on with `=` as one argv token. A space-separated `--force-with-lease
            // <value>` form (two argv entries) makes git treat the flag as valueless and
            // silently shifts `<value>` into the remote-name positional instead — verified
            // during the earlier design review, which is why this is one combined token.
            let lease = format!("--force-with-lease=refs/heads/{apply_ref}:{expected}");
            let out = tokio::process::Command::new("git")
                .arg("-C").arg(dir)
                .args(["push", &lease, "origin", &format!(":refs/heads/{apply_ref}")])
                .output().await;
            match out {
                Ok(o) if o.status.success() => remote_deleted = true,
                Ok(o) => {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    if stderr.contains("stale info") || stderr.contains("rejected") {
                        return nave_apply::ResetRepoResult { repo: req.repo.clone(), local_reset, remote_deleted: false, state: nave_apply::ResetState::RemoteCasMismatch, reason: Some("remote apply branch has moved since it was pushed — left intact".into()) };
                    }
                    reason = Some(format!("remote delete failed: {}", stderr.trim()));
                }
                Err(e) => reason = Some(format!("remote delete failed: {e}")),
            }
        }
        // expected_pushed_sha == None: never pushed, nothing remote to clean up — idempotent.
    }

    let state = if reason.is_some() { nave_apply::ResetState::MissingBranch } else { nave_apply::ResetState::Ok };
    nave_apply::ResetRepoResult { repo: req.repo.clone(), local_reset, remote_deleted, state, reason }
}
```

CLI wiring mirrors Task 8's `push` command shape (`name`, `branch` positional, `--request`, `--json`, same parse-error-to-envelope wrapper).

- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit** — `feat: nave pen reset verb (atomic CAS-guarded cleanup)`.

---

### Task 10: End-to-end integration proof (real clones, real remote)

**Files:** Create `crates/nave/tests/pen_apply.rs`; Modify `crates/nave/Cargo.toml` (`[dev-dependencies]`: `nave_test_support`, `nave_apply`, `nave_pen`).

**Interfaces:** drives the **real CLI binary** (`CARGO_BIN_EXE_nave`, matching `smoke.rs`'s existing pattern) through `capabilities → branch → commit → push → reset`, writing real request files to a tempdir and parsing real stdout JSON. Wires the CLI's config resolution to the fixture's `pen_root` by writing a real `nave.toml` under `$HOME/.config/` (mirroring `smoke.rs`'s existing `write_config` helper for `api_base`) — this is worked out concretely here, not left as an open gap for the implementer to resolve, since Task 1's fixture already writes `pen.toml` via `write_pen` and checks out `pen.branch`, so `load_pen`/`resolve_pen_root` need only be pointed at the right root.

- [ ] **Step 1: Write failing test**

```rust
// crates/nave/tests/pen_apply.rs
use std::process::Command;

fn write_pen_config(home: &std::path::Path, pen_root: &std::path::Path) {
    let config_dir = home.join(".config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("nave.toml"),
        format!("[pen]\nroot = {:?}\n", pen_root.to_string_lossy()),
    ).unwrap();
}

fn nave(args: &[&str], home: &std::path::Path) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_nave")).args(args).env("HOME", home).output().unwrap();
    assert!(out.status.success(), "nave {args:?} exited {:?}: {}", out.status, String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("bad json from nave {args:?}: {e}: {}", String::from_utf8_lossy(&out.stdout)))
}

#[tokio::test]
async fn full_apply_lifecycle_lands_on_real_clone_and_cleans_up() {
    let fx = nave_test_support::init_pen_fixture("e2e-apply", "acme", "docs", "develop").await;
    let home = tempfile::TempDir::new().unwrap();
    write_pen_config(home.path(), fx.pen_root.path());

    let caps = nave(&["pen", "capabilities", "--json"], home.path());
    assert_eq!(caps["verbs"].as_array().unwrap().len(), 4);

    let reqdir = tempfile::TempDir::new().unwrap();
    let branch_req_path = reqdir.path().join("branch.json");
    std::fs::write(&branch_req_path, format!(
        r#"{{"protocol_version":1,"apply_ref":"pulse/apply/e2e","repos":[{{"repo":"acme/docs","base_ref":"develop","expected_base_sha":"{}"}}]}}"#,
        fx.base_sha,
    )).unwrap();
    let branch_res = nave(&["pen", "branch", "e2e-apply", "--request", branch_req_path.to_str().unwrap(), "--json"], home.path());
    assert_eq!(branch_res["repos"][0]["state"], "ok");

    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "e2e-apply", "acme", "docs");
    std::fs::write(dir.join("lockfile.json"), "{}").unwrap();

    let commit_req_path = reqdir.path().join("commit.json");
    std::fs::write(&commit_req_path, r#"{"protocol_version":1,"repos":[{"repo":"acme/docs","paths":["lockfile.json"]}]}"#).unwrap();
    let commit_res = nave(&["pen", "commit", "e2e-apply", "pulse/apply/e2e", "--request", commit_req_path.to_str().unwrap(), "-m", "bump lockfile", "--json"], home.path());
    assert_eq!(commit_res["repos"][0]["state"], "ok");
    let local_sha = commit_res["repos"][0]["local_commit_sha"].as_str().unwrap().to_string();

    let push_req_path = reqdir.path().join("push.json");
    std::fs::write(&push_req_path, r#"{"protocol_version":1,"repos":[{"repo":"acme/docs"}]}"#).unwrap();
    let push_res = nave(&["pen", "push", "e2e-apply", "pulse/apply/e2e", "--request", push_req_path.to_str().unwrap(), "--json"], home.path());
    assert_eq!(push_res["repos"][0]["state"], "ok");
    assert_eq!(push_res["repos"][0]["remote_sha"], local_sha);

    let reset_req_path = reqdir.path().join("reset.json");
    std::fs::write(&reset_req_path, format!(r#"{{"protocol_version":1,"repos":[{{"repo":"acme/docs","expected_pushed_sha":"{local_sha}"}}]}}"#)).unwrap();
    let reset_res = nave(&["pen", "reset", "e2e-apply", "pulse/apply/e2e", "--request", reset_req_path.to_str().unwrap(), "--json"], home.path());
    assert_eq!(reset_res["repos"][0]["state"], "ok");
    assert_eq!(reset_res["repos"][0]["remote_deleted"], true);

    let remote_refs = nave_pen::apply_ops::capabilities(); // sanity: crate still loads after full lifecycle
    assert!(!remote_refs.verbs.is_empty());
}
```

- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** — no production code changes expected if Tasks 1-9 wired the CLI correctly; this task's "implementation" is confirming `resolve_pen_root`/`load_default` actually honor `$HOME/.config/nave.toml`'s `[pen] root` (check `nave_config::load_default`/`resolve_pen_root` — if the config key path differs from `[pen] root`, fix `write_pen_config` to match the real schema, not the other way around) and closing any gap that surfaces.
- [ ] **Step 4: Run, verify pass** (`cargo test -p nave --test pen_apply`).
- [ ] **Step 5: Commit** — `test: end-to-end apply-verb lifecycle against real clones`.

---

### Task 11: Docs

**Files:** Modify `docs/reference/cli/pen.md` (overview — add the five verbs to the command list); Create `docs/reference/cli/pen/branch.md`, `docs/reference/cli/pen/commit.md`, `docs/reference/cli/pen/push.md` (distinct from `pen exec --push-changes`, unrelated — note the distinction explicitly in the page), `docs/reference/cli/pen/reset.md`, `docs/reference/cli/pen/capabilities.md`.

- [ ] **Step 1:** For each new verb, write a real reference page following the existing `docs/reference/cli/pen/status.md`/`exec.md` structure (command synopsis, flags table, JSON shape, one example invocation + example output). No stub placeholders.
- [ ] **Step 2:** Update `docs/reference/cli/pen.md`'s command list to include the five new subcommands with one-line descriptions matching the `PenAction` doc comments from Tasks 5-9.
- [ ] **Step 3:** Run `just build-docs` (verify it completes without network access in this environment; if it requires network, note that explicitly rather than silently skipping verification) and **commit** — `docs: reference pages for the five apply verbs`.

---

### Task 12: Coordination handoff note for `hiivmind-pulse-gh`

**Files:** Create `docs/superpowers/specs/2026-08-13-apply-verb-contract-handoff.md`.

- [ ] **Step 1:** Write a short note recording: the final JSON wire shape for all five verbs as shipped (copy the `nave_apply` types verbatim, post-implementation, in case any detail shifted during Task 3's implementation); the five coordination points already listed in this plan's "Coordination with hiivmind-pulse-gh" section; the CLI invocation shapes (`nave pen branch <name> --request <file> --json`, etc., including that `branch`'s request carries `apply_ref` at the envelope level while `commit`/`push`/`reset` take it as a `<branch>` positional) the Python `nave_adapter.py` argv-builders must match exactly; and an explicit instruction that `hiivmind-pulse-gh`'s Task 1 implementer should read this file, not re-derive the contract from the (now superseded-in-detail) Authoritative Interfaces table alone.
- [ ] **Step 2: Commit** — `docs: apply-verb contract handoff note for hiivmind-pulse-gh Task 1`.

---

## Completion note

A green suite here proves the five verbs work against real, local git remotes — the "Rust half" of F11 production wiring (design spec §8 step 1 of 3). It does **not** land apply-mode end-to-end: `hiivmind-pulse-gh`'s own 12-task plan (Task 1 onward) still has to consume this contract, delete its raw-git trio, and build the driver; `hiivmind/hiivmind-workspace` enrollment is last. Do not report this plan's completion as "F11 shipped."

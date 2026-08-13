# Nave Apply-Verb Extensions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add five `nave pen` verbs — `capabilities`, `branch`, `commit`, `push`, `reset` — implementing the apply-mode git-mutation contract that `hiivmind-pulse-gh`'s Path-A apply driver consumes. Nave becomes the **sole** clone mutator for apply-mode landings; `hiivmind-pulse-gh` keeps orchestration, policy, and read-only verification only.

**Architecture:** A new contract-only crate `nave_apply` (mirrors the existing `nave_materialize` crate: versioned envelopes, `deny_unknown_fields`, deterministic serialization, no git/network code) defines the wire types every verb speaks. A new `nave_pen::apply_ops` module (mirrors the existing `ops.rs` mutation idiom) implements the git plumbing against those types, using two new internal helpers: `git_util` (captured-stdout git invocation — `ops.rs`'s existing helpers are fire-and-forget and don't return output) and `apply_state` (a per-pen, per-apply-branch sidecar recording the provisioned base SHA across the separate process invocations each verb runs as — mirrors the existing `rewrite_state.rs` sidecar pattern in this same crate). CLI wiring extends `PenAction` in `crates/nave/src/commands/pen.rs` with five new subcommands, following the existing `--request <file> --json` idiom from `nave materialize`.

**Tech Stack:** Rust workspace (edition 2024), `clap` derive, `serde`/`serde_json`, `tokio` (process), `anyhow`, `thiserror`, `tracing`, `cargo nextest`. One new dev-only dependency: `tempfile` (for real-git-repo fixtures — this plan's tests are the first in the codebase to exercise git mutation against real repos rather than mocked HTTP).

**Source spec:** `hiivmind-pulse-gh`'s `docs/superpowers/specs/2026-07-30-apply-mode-production-wiring-design.md` §3 (already adversarially reviewed twice, Codex `gpt-5.6-sol`) is this plan's design spec — read it first. `hiivmind-pulse-gh`'s `docs/superpowers/plans/2026-07-30-apply-mode-pulse-wiring.md` Task 1 is the **Python-side contract this plan must satisfy**: its Authoritative Interfaces table specifies the five `nave_adapter.py` function signatures (`pen_capabilities`, `pen_branch`, `pen_commit`, `pen_push`, `pen_reset`) that decode exactly the JSON this plan's verbs emit. Where that table leaves a detail unspecified (per-repo `state` enum values, the request shape a given verb needs), **this plan is the first to implement and therefore the first to fix that detail** — see "Coordination with hiivmind-pulse-gh" below.

**Verified against the current repo (no drift as of 2026-08-13):** `crates/nave/src/commands/pen.rs`'s `PenAction` enum has `Create/List/Show/Status/Sync/Clean/Revert/Reinit/Exec/Rm/Rewrite` only — none of these five verbs exist yet, confirming the pulse-gh plan's assumption that Task 1's contract is entirely net-new on this side.

## Global Constraints

- **No new production runtime dependencies.** Reuse `anyhow`, `thiserror`, `serde`, `serde_json`, `tokio`, `clap`, `tracing`, `time`, already in `[workspace.dependencies]`. `tempfile` is dev-only.
- **Every verb request and result carries `protocol_version` (currently `1`, `nave_apply::PROTOCOL_VERSION`).** A request whose `protocol_version` doesn't match is an envelope-level `error`, never silently accepted.
- **Envelope `adapter_state` is `"ok"` or `"error"` only** — `"ok"` means the command ran and every requested repo got a determinate per-repo outcome (even if that outcome is itself a controlled failure, e.g. `stale-base`). `"error"` is reserved for envelope-level failures the caller cannot attribute to a single repo: bad `protocol_version`, malformed/empty request, duplicate or non-`owner/name` repo identity, coverage mismatch.
- **Per-repo `state` is a closed `serde` enum, never a free string** — an invalid value is a deserialization error on the Python side by construction (the pulse-gh Task 1 tests assert exactly this: "state enum invalid → error").
- **Exact request-repo coverage.** Every verb's result contains exactly one entry per repo named in the request — no extra, no missing, no duplicate. Coverage is validated before any git command runs.
- **Fail closed.** A missing clone directory, a failed git command, or a violated invariant is a per-repo failure state — never a fabricated `"ok"`.
- **This plan does not delete `hiivmind-pulse-gh`'s raw-git trio** (`provision_apply_branch`/`commit_apply_clones`/`push_apply_clones` in `nave_adapter.py`). That deletion is pulse-gh's own Task 1, gated on this plan shipping — out of scope here.
- **`just test` (`cargo nextest run --no-fail-fast`) and `just lint` (`cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --all -- --check`) pass before every task's commit.**
- Land on `master` — confirmed as this fork's PR-integration branch (PR #1 merged to `master`; `origin/develop` here tracks the upstream project's own line and carries unrelated history, e.g. `#18` "Pen rewriting", `#19`/`#20` pre-commit bumps — not this fork's work).

## Authoritative Interfaces (single source of truth — every task and test uses these verbatim)

```rust
// nave_apply (Task 3) — contract-only crate, no git/network, mirrors nave_materialize
pub const PROTOCOL_VERSION: u32 = 1;
pub const APPLY_VERBS: &[&str] = &["branch", "commit", "push", "reset"];

pub enum AdapterState { Ok, Error }                          // #[serde(rename_all = "snake_case")]

pub struct CapabilitiesResult {
    pub protocol_version: u32,
    pub verbs: Vec<String>,
    pub adapter_state: AdapterState,
    pub reason: Option<String>,
}

pub struct BranchEnvelope { pub protocol_version: u32, pub repos: Vec<BranchRepoRequest> }
pub struct BranchRepoRequest { pub repo: String, pub base_ref: String, pub expected_base_sha: String, pub apply_ref: String }
pub enum BranchState { Ok, StaleBase, Exists, MissingRef, NotACommit }   // #[serde(rename_all = "kebab-case")]
pub struct BranchRepoResult { pub repo: String, pub base_ref: String, pub expected_base_sha: String, pub observed_base_sha: String, pub apply_ref: String, pub state: BranchState, pub reason: Option<String> }
pub struct BranchResult { pub protocol_version: u32, pub adapter_state: AdapterState, pub repos: Vec<BranchRepoResult> }

pub struct CommitEnvelope { pub protocol_version: u32, pub message: String, pub repos: Vec<CommitRepoRequest> }
pub struct CommitRepoRequest { pub repo: String, pub paths: Vec<String> }
pub enum CommitState { Ok, NothingToCommit, DirtyOutsideBounds, InvariantViolated, MissingClone, NoApplyState }
pub struct CommitRepoResult { pub repo: String, pub local_commit_sha: Option<String>, pub state: CommitState, pub reason: Option<String> }
pub struct CommitResult { pub protocol_version: u32, pub adapter_state: AdapterState, pub repos: Vec<CommitRepoResult> }

pub struct PushEnvelope { pub protocol_version: u32, pub repos: Vec<PushRepoRequest> }
pub struct PushRepoRequest { pub repo: String }
pub enum PushState { Ok, MissingBranch, Diverged, PushRejected, NoApplyState }
pub struct PushRepoResult { pub repo: String, pub remote: Option<String>, pub remote_ref: Option<String>, pub remote_sha: Option<String>, pub upstream: Option<String>, pub local_commit_sha: Option<String>, pub state: PushState, pub reason: Option<String> }
pub struct PushResult { pub protocol_version: u32, pub adapter_state: AdapterState, pub repos: Vec<PushRepoResult> }

pub struct ResetEnvelope { pub protocol_version: u32, pub repos: Vec<ResetRepoRequest> }
pub struct ResetRepoRequest { pub repo: String, pub expected_pushed_sha: Option<String> }  // None = never pushed; skip remote CAS delete
pub enum ResetState { Ok, RemoteCasMismatch, MissingBranch }
pub struct ResetRepoResult { pub repo: String, pub local_reset: bool, pub remote_deleted: bool, pub state: ResetState, pub reason: Option<String> }
pub struct ResetResult { pub protocol_version: u32, pub adapter_state: AdapterState, pub repos: Vec<ResetRepoResult> }

#[derive(thiserror::Error, Debug)]
pub enum ValidationError { /* EmptyRepos, ProtocolVersionMismatch(u32), DuplicateRepo(String), InvalidRepoIdentity(String) — Task 3 */ }
pub fn validate_envelope_repos(protocol_version: u32, repos: &[String]) -> Result<(), ValidationError>;  // Task 3

// nave_pen::apply_state (Task 4, pub(crate) — internal sidecar, not part of the wire contract)
pub(crate) struct ApplyState { pub repos: BTreeMap<String, ApplyRepoState> }         // key = "owner/name"
pub(crate) struct ApplyRepoState { pub base_ref: String, pub expected_base_sha: String, pub local_commit_sha: Option<String> }
pub(crate) fn read_apply_state(pen_root: &Path, pen_name: &str, apply_ref: &str) -> Result<ApplyState>;
pub(crate) fn write_apply_state(pen_root: &Path, pen_name: &str, apply_ref: &str, state: &ApplyState) -> Result<()>;
pub(crate) fn clear_apply_state(pen_root: &Path, pen_name: &str, apply_ref: &str) -> Result<()>;

// nave_pen::git_util (Task 4, pub(crate))
pub(crate) async fn git_output(dir: &Path, args: &[&str]) -> Result<String>;   // captured, trimmed stdout; bail with stderr on nonzero exit
pub(crate) async fn git_status(dir: &Path, args: &[&str]) -> Result<()>;       // side-effect only, discards stdout
pub(crate) async fn git_ok(dir: &Path, args: &[&str]) -> Result<bool>;         // existence probes — nonzero exit is `false`, not an error

// nave_pen::apply_ops (Tasks 5-9) — the five verb implementations
pub fn capabilities() -> CapabilitiesResult;
pub async fn provision_branch(pen_root: &Path, pen: &Pen, request: &BranchEnvelope) -> Result<BranchResult>;
pub async fn commit_bound(pen_root: &Path, pen: &Pen, apply_ref: &str, request: &CommitEnvelope) -> Result<CommitResult>;
pub async fn push_branch(pen_root: &Path, pen: &Pen, apply_ref: &str, request: &PushEnvelope) -> Result<PushResult>;
pub async fn reset_branch(pen_root: &Path, pen: &Pen, apply_ref: &str, request: &ResetEnvelope) -> Result<ResetResult>;

// nave_test_support (Task 1, dev-dependency only)
pub struct PenFixture { pub origin: TempDir, pub pen_root: TempDir, pub pen: Pen, pub base_sha: String }
pub async fn init_pen_fixture(pen_name: &str, owner: &str, repo: &str, default_branch: &str) -> PenFixture;
```

### Coordination with `hiivmind-pulse-gh` (read before implementing Task 1's Python adapters)

This plan fixes three details the pulse-gh plan's Task 1 table left open, because this side implements first:

1. **Per-repo `state` enums** (listed above per verb) — the pulse-gh `nave_adapter.py` `_validate_apply_result` helper's "state enum invalid" test must check membership against these exact kebab-case strings (e.g. `"stale-base"`, `"dirty-outside-bounds"`, `"remote-cas-mismatch"`), not an unspecified placeholder set.
2. **`pen_commit`'s request shape carries only `{repo, paths}`, not `expected_base_sha`.** The base-SHA invariant check happens Nave-side against the `apply_state` sidecar `provision_branch` wrote — the Python driver does not need to (and should not) resend it.
3. **`pen_status --json` gains `clone_path: Option<String>` per repo** (Task 2) — `null` when the clone directory doesn't exist, matching the pulse-gh plan's Task 1 note ("`pen_status` … only **adds** `clone_path`").

When `hiivmind-pulse-gh`'s Task 1 lands, its Authoritative Interfaces table should be amended to cite this file rather than leaving the per-repo state enums unspecified — flagged for that plan's own review, not actioned here (cross-repo doc edits stay in their own repo).

---

### Task 1: `nave_test_support` — real-git fixture crate

**Files:** Create `crates/nave_test_support/Cargo.toml`, `crates/nave_test_support/src/lib.rs`; Modify `Cargo.toml` (workspace: add `nave_test_support` to `[workspace.dependencies]`, add `tempfile = {version = "3"}`); Modify `crates/nave_pen/Cargo.toml` (dev-dependency: `tempfile`, `nave_test_support`, `tokio` with `["rt-multi-thread","macros"]` test features via workspace); Modify `crates/nave/Cargo.toml` (dev-dependency: `nave_test_support`).

**Interfaces:** `init_pen_fixture` from the table. Builds a **bare** repo in one tempdir (acts as `origin`), seeds one commit on `default_branch`, records its SHA, clones it into a second tempdir laid out exactly like `pen_repo_clone_dir` expects (`<pen_root>/pens/<pen_name>/repos/<owner>__<repo>/`), and returns a `Pen` (from `nave_pen::storage`) whose single `PenRepo` points at that clone with `clone_url` set to the bare repo's `file://` path. This is the first git-mutation test fixture in the codebase (existing tests only mock HTTP for `materialize`/`scan`) — every later task's tests build on it.

- [ ] **Step 1: Write failing test**

```rust
// crates/nave_test_support/src/lib.rs — the crate's own doctest-style smoke check, run as a unit test
#[tokio::test]
async fn fixture_clone_has_committed_default_branch_head() {
    let fx = init_pen_fixture("apply-fixture", "acme", "docs", "develop").await;
    let head = tokio::process::Command::new("git")
        .arg("-C").arg(fx.pen.repos[0].clone_url.trim_start_matches("file://"))
        .args(["rev-parse", "HEAD"])
        .output()
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), fx.base_sha);
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
    std::fs::create_dir_all(clone_dir.parent().unwrap()).unwrap();
    git(pen_root.path(), &["clone", origin.path().to_str().unwrap(), clone_dir.to_str().unwrap()]).await;

    let pen = Pen {
        name: pen_name.to_string(),
        created_at: OffsetDateTime::now_utc(),
        branch: format!("nave/{pen_name}"),
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

    PenFixture { origin, pen_root, pen, base_sha }
}
```

- [ ] **Step 4: Run, verify pass** — `cargo test -p nave_test_support`.
- [ ] **Step 5: Commit** — `test: real-git pen fixture crate for apply-verb tests`.

---

### Task 2: `pen status --json` gains `clone_path`

**Files:** Modify `crates/nave_pen/src/state.rs`; Create `crates/nave_pen/tests/status_clone_path.rs`.

**Interfaces:** `RepoState` gains `pub clone_path: Option<String>` (`None` when the clone directory doesn't exist — matches the existing "Missing" `WorkTree` early-return branch). This is spec §3 item 6 (the read-path capability `pen_clone_reader` needs) — small and independent of the other four tasks, land it first since it touches no new module.

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

**Interfaces:** every type in the Authoritative Interfaces table's `nave_apply` block. Mirrors `nave_materialize`'s conventions exactly: `#[serde(deny_unknown_fields)]` on every request type, a custom `Serialize` impl on envelope types that sorts `repos` lexically by `repo` before emitting (deterministic diffs), `#[serde(rename_all = "kebab-case")]` on state enums (so a state value in JSON reads `"stale-base"`, matching this plan's coordination note), `#[serde(rename_all = "snake_case")]` on `AdapterState`. `validate_envelope_repos` is generic over the four request kinds by taking only what varies (`protocol_version`, the `repo` identity list) — each verb's caller extracts that list before calling it.

- [ ] **Step 1: Write failing tests**

```rust
// crates/nave_apply/tests/contract.rs
use nave_apply::{
    AdapterState, BranchEnvelope, BranchRepoRequest, BranchState, PROTOCOL_VERSION,
    ValidationError, validate_envelope_repos,
};

fn branch_req(repo: &str) -> BranchRepoRequest {
    BranchRepoRequest {
        repo: repo.into(), base_ref: "develop".into(),
        expected_base_sha: "a".repeat(40), apply_ref: "pulse/apply/p1".into(),
    }
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
fn unknown_keys_in_branch_request_are_rejected() {
    let raw = r#"{"protocol_version":1,"repos":[{"repo":"a/b","base_ref":"main","expected_base_sha":"x","apply_ref":"y","extra":true}]}"#;
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
    let e1 = BranchEnvelope { protocol_version: 1, repos: vec![branch_req("z/z"), branch_req("a/a")] };
    let e2 = BranchEnvelope { protocol_version: 1, repos: vec![branch_req("a/a"), branch_req("z/z")] };
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
// crates/nave_apply/src/lib.rs (excerpt — full file follows this shape for Commit/Push/Reset)
use serde::{Deserialize, Serialize, Serializer};

pub const PROTOCOL_VERSION: u32 = 1;
pub const APPLY_VERBS: &[&str] = &["branch", "commit", "push", "reset"];

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
    pub apply_ref: String,
}

impl Serialize for BranchRepoRequest {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("BranchRepoRequest", 4)?;
        st.serialize_field("repo", &self.repo)?;
        st.serialize_field("base_ref", &self.base_ref)?;
        st.serialize_field("expected_base_sha", &self.expected_base_sha)?;
        st.serialize_field("apply_ref", &self.apply_ref)?;
        st.end()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchEnvelope {
    pub protocol_version: u32,
    pub repos: Vec<BranchRepoRequest>,
}

impl Serialize for BranchEnvelope {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut repos = self.repos.clone();
        repos.sort_by(|a, b| a.repo.cmp(&b.repo));
        let mut st = s.serialize_struct("BranchEnvelope", 2)?;
        st.serialize_field("protocol_version", &self.protocol_version)?;
        st.serialize_field("repos", &repos)?;
        st.end()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BranchState { Ok, StaleBase, Exists, MissingRef, NotACommit }

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
    pub repos: Vec<BranchRepoResult>,
}

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
```

Repeat the `RepoRequest`/`Envelope`/`State`/`RepoResult`/`Result` quintet for `Commit`, `Push`, `Reset` exactly per the Authoritative Interfaces table's field lists, each with the same `deny_unknown_fields` request / sorted-`Serialize` envelope / kebab-case state / `skip_serializing_if` optional-field pattern shown above for `Branch`.

- [ ] **Step 4: Run, verify pass** (`cargo test -p nave_apply`).
- [ ] **Step 5: Commit** — `feat: nave_apply contract crate (capabilities/branch/commit/push/reset)`.

---

### Task 4: `git_util` + `apply_state` shared modules

**Files:** Create `crates/nave_pen/src/git_util.rs`, `crates/nave_pen/src/apply_state.rs`, `crates/nave_pen/tests/apply_state.rs`; Modify `crates/nave_pen/src/lib.rs` (add `mod git_util; mod apply_state;` — both stay private to the crate, no `pub use`); Modify `crates/nave_pen/Cargo.toml` (add `toml = {workspace = true}` if not already present — it is, via existing `[dependencies] toml`).

**Interfaces:** `git_output`/`git_status`/`git_ok` and the `ApplyState` sidecar from the table. The sidecar exists because `provision_branch`, `commit_bound`, `push_branch`, `reset_branch` run as **separate CLI process invocations** — nothing survives in memory between them, so the base SHA `provision_branch` verified must be persisted for `commit_bound` to re-check its "HEAD hasn't drifted since provisioning" invariant (design spec §3 item 5). Stored at `<pen_dir>/apply/<apply_ref-with-slashes-replaced-by-__>.toml`, written by `provision_branch`, updated by `commit_bound` (adds `local_commit_sha`), read by `push_branch`, cleared by `reset_branch`.

- [ ] **Step 1: Write failing tests**

```rust
// crates/nave_pen/tests/apply_state.rs — exercises the pub(crate) module via a thin pub(crate)-visible
// re-export gate isn't available cross-crate, so this test lives as a #[cfg(test)] unit test instead:
// see git_util.rs / apply_state.rs inline `mod tests` below (Step 3) — this file intentionally stays
// empty of assertions and is removed once the inline tests land; committed as a placeholder marker
// is prohibited, so skip creating this file and add the tests inline in Step 3 instead.
```

(Skip the separate integration-test file — `apply_state`/`git_util` are `pub(crate)`, not reachable from `crates/nave_pen/tests/*.rs`. Tests go inline as `#[cfg(test)] mod tests` inside each new module, consistent with idiomatic Rust for crate-private code; this is a plan correction from the initial file list above.)

```rust
// crates/nave_pen/src/apply_state.rs — bottom of file
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_disk() {
        let root = tempfile::TempDir::new().unwrap();
        let mut state = ApplyState::default();
        state.repos.insert(
            "acme/docs".into(),
            ApplyRepoState { base_ref: "develop".into(), expected_base_sha: "a".repeat(40), local_commit_sha: None },
        );
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
//! Per-pen, per-apply-branch sidecar: the base SHA `provision_branch`
//! verified, persisted because each verb runs as a separate process.

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
    #[serde(default)]
    pub local_commit_sha: Option<String>,
}

fn apply_state_path(pen_root: &Path, pen_name: &str, apply_ref: &str) -> PathBuf {
    pen_dir(pen_root, pen_name).join("apply").join(format!("{}.toml", apply_ref.replace('/', "__")))
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
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, toml::to_string_pretty(state)?).with_context(|| format!("writing {}", path.display()))
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

**Files:** Create `crates/nave_pen/src/apply_ops.rs`; Modify `crates/nave_pen/src/lib.rs` (`pub mod apply_ops;` + re-export `capabilities`); Modify `crates/nave/src/commands/pen.rs` (`PenAction::Capabilities`, `PenCapabilitiesArgs`, `run_capabilities`); Modify `crates/nave/Cargo.toml` (`nave_apply = {workspace = true}`); Modify root `Cargo.toml` `[workspace.dependencies]` in `nave_pen`'s and `nave`'s own `Cargo.toml` (`nave_apply = {workspace = true}`).

**Interfaces:** `apply_ops::capabilities() -> CapabilitiesResult` from the table — pure, synchronous, no I/O; a stale Nave binary that predates this verb simply doesn't have the `capabilities` subcommand at all, so the pulse-gh handshake's "missing verb" failure mode is `clap`'s own "unrecognized subcommand" nonzero exit, not something this verb needs to simulate.

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

Add `pub use apply_ops::capabilities;` alongside the existing re-exports in `crates/nave_pen/src/lib.rs`, and `pub mod apply_ops;` (the four remaining functions land there in Tasks 6-9 and are re-exported the same way once implemented).

- [ ] **Step 4: Run, verify pass** (`cargo test -p nave_pen` + `cargo run --bin nave -- pen capabilities --json` prints the expected object).
- [ ] **Step 5: Commit** — `feat: nave pen capabilities verb`.

---

### Task 6: `nave pen branch` verb — remote-base CAS provisioning

**Files:** Modify `crates/nave_pen/src/apply_ops.rs`; Modify `crates/nave/src/commands/pen.rs` (`PenAction::Branch`, `PenBranchArgs`, `run_branch`).

**Interfaces:** `apply_ops::provision_branch` from the table. Per repo: fetch the named `base_ref` fresh from `origin` (never trust a stale local ref — design spec §3 item 1), compare the observed remote SHA to `expected_base_sha` (a mismatch is `stale-base`, not a hard error — CAS failure is an expected, reportable outcome), verify the resolved object is a commit, fail closed if `apply_ref` already exists locally (never blind reuse — reuse decisions live in the pulse-gh driver's journal, not here), then `checkout -B` off the verified SHA and persist the provisioned base into the `apply_state` sidecar for `commit_bound` to check later.

- [ ] **Step 1: Write failing tests**

```rust
// crates/nave_pen/src/apply_ops.rs — add to the existing `mod tests`
#[tokio::test]
async fn branch_provisions_off_verified_remote_base() {
    let fx = nave_test_support::init_pen_fixture("branch-fx", "acme", "docs", "develop").await;
    let req = nave_apply::BranchEnvelope {
        protocol_version: nave_apply::PROTOCOL_VERSION,
        repos: vec![nave_apply::BranchRepoRequest {
            repo: "acme/docs".into(), base_ref: "develop".into(),
            expected_base_sha: fx.base_sha.clone(), apply_ref: "pulse/apply/p1".into(),
        }],
    };
    let res = provision_branch(fx.pen_root.path(), &fx.pen, &req).await.unwrap();
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
    let req = nave_apply::BranchEnvelope {
        protocol_version: nave_apply::PROTOCOL_VERSION,
        repos: vec![nave_apply::BranchRepoRequest {
            repo: "acme/docs".into(), base_ref: "develop".into(),
            expected_base_sha: "0".repeat(40), apply_ref: "pulse/apply/p1".into(),
        }],
    };
    let res = provision_branch(fx.pen_root.path(), &fx.pen, &req).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::BranchState::StaleBase));
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "branch-fx2", "acme", "docs");
    let branch = crate::git_util::git_output(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await.unwrap();
    assert_eq!(branch, "develop");
}

#[tokio::test]
async fn branch_fails_closed_when_apply_ref_already_exists() {
    let fx = nave_test_support::init_pen_fixture("branch-fx3", "acme", "docs", "develop").await;
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "branch-fx3", "acme", "docs");
    crate::git_util::git_status(&dir, &["checkout", "-B", "pulse/apply/p1"]).await.unwrap();
    crate::git_util::git_status(&dir, &["checkout", "develop"]).await.unwrap();

    let req = nave_apply::BranchEnvelope {
        protocol_version: nave_apply::PROTOCOL_VERSION,
        repos: vec![nave_apply::BranchRepoRequest {
            repo: "acme/docs".into(), base_ref: "develop".into(),
            expected_base_sha: fx.base_sha.clone(), apply_ref: "pulse/apply/p1".into(),
        }],
    };
    let res = provision_branch(fx.pen_root.path(), &fx.pen, &req).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::BranchState::Exists));
}
```

- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement.**

```rust
// crates/nave_pen/src/apply_ops.rs — append
use std::path::Path;

use crate::apply_state::{ApplyRepoState, ApplyState, write_apply_state};
use crate::git_util::{git_ok, git_output, git_status};
use crate::storage::pen_repo_clone_dir;
use crate::Pen;

fn split_repo(repo: &str) -> (&str, &str) {
    let mut parts = repo.splitn(2, '/');
    (parts.next().unwrap_or_default(), parts.next().unwrap_or_default())
}

pub async fn provision_branch(
    pen_root: &Path,
    pen: &Pen,
    request: &nave_apply::BranchEnvelope,
) -> anyhow::Result<nave_apply::BranchResult> {
    let repo_ids: Vec<String> = request.repos.iter().map(|r| r.repo.clone()).collect();
    if let Err(e) = nave_apply::validate_envelope_repos(request.protocol_version, &repo_ids) {
        return Ok(nave_apply::BranchResult {
            protocol_version: nave_apply::PROTOCOL_VERSION,
            adapter_state: nave_apply::AdapterState::Error,
            repos: vec![],
        });
        // caller logs `e` — see CLI wiring below, which prints the validation reason before returning this envelope.
        let _ = e;
    }

    let mut results = Vec::with_capacity(request.repos.len());
    let mut apply_state = ApplyState::default();
    for req in &request.repos {
        let (owner, name) = split_repo(&req.repo);
        let dir = pen_repo_clone_dir(pen_root, &pen.name, owner, name);
        let result = provision_one(&dir, req).await;
        if matches!(result.state, nave_apply::BranchState::Ok) {
            apply_state.repos.insert(
                req.repo.clone(),
                ApplyRepoState { base_ref: req.base_ref.clone(), expected_base_sha: req.expected_base_sha.clone(), local_commit_sha: None },
            );
        }
        results.push(result);
    }
    if !apply_state.repos.is_empty() {
        let apply_ref = &request.repos[0].apply_ref;
        write_apply_state(pen_root, &pen.name, apply_ref, &apply_state)?;
    }

    Ok(nave_apply::BranchResult { protocol_version: nave_apply::PROTOCOL_VERSION, adapter_state: nave_apply::AdapterState::Ok, repos: results })
}

async fn provision_one(dir: &Path, req: &nave_apply::BranchRepoRequest) -> nave_apply::BranchRepoResult {
    let mk = |state, observed: String, reason: Option<&str>| nave_apply::BranchRepoResult {
        repo: req.repo.clone(), base_ref: req.base_ref.clone(), expected_base_sha: req.expected_base_sha.clone(),
        observed_base_sha: observed, apply_ref: req.apply_ref.clone(), state, reason: reason.map(str::to_string),
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
    if git_ok(dir, &["rev-parse", "--verify", "--quiet", &req.apply_ref]).await.unwrap_or(false) {
        return mk(nave_apply::BranchState::Exists, observed, Some("apply branch already exists"));
    }
    if let Err(e) = git_status(dir, &["checkout", "-B", &req.apply_ref, &observed]).await {
        return mk(nave_apply::BranchState::NotACommit, observed, Some(&e.to_string()));
    }
    mk(nave_apply::BranchState::Ok, observed, None)
}
```

Fix the `Step 3` sketch's validation branch (it must not both `return` and reference `e` afterward — the review below flags exactly this class of issue; the corrected shape returns the error envelope carrying the reason on the *single* early-exit path):

```rust
    if let Err(e) = nave_apply::validate_envelope_repos(request.protocol_version, &repo_ids) {
        return Ok(nave_apply::BranchResult {
            protocol_version: nave_apply::PROTOCOL_VERSION,
            adapter_state: nave_apply::AdapterState::Error,
            repos: vec![nave_apply::BranchRepoResult {
                repo: String::new(), base_ref: String::new(), expected_base_sha: String::new(),
                observed_base_sha: String::new(), apply_ref: String::new(),
                state: nave_apply::BranchState::MissingRef, reason: Some(e.to_string()),
            }],
        });
    }
```

CLI wiring in `crates/nave/src/commands/pen.rs`, following the `materialize.rs` `--request FILE` idiom (never inline JSON on the command line):

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
    let request: nave_apply::BranchEnvelope = serde_json::from_str(&raw).context("parsing request")?;
    let result = nave_pen::apply_ops::provision_branch(&root, &pen, &request).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    Ok(())
}
```

Add `Branch(PenBranchArgs)` to `PenAction`, `PenAction::Branch(a) => run_branch(a).await,` to `run`, and `pub use apply_ops::provision_branch;` to `nave_pen`'s `lib.rs`.

- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit** — `feat: nave pen branch verb (remote-base CAS provisioning)`.

---

### Task 7: `nave pen commit` verb — bounded staging + post-exec invariants

**Files:** Modify `crates/nave_pen/src/apply_ops.rs`; Modify `crates/nave/src/commands/pen.rs` (`PenAction::Commit`, `PenCommitArgs`, `run_commit`).

**Interfaces:** `apply_ops::commit_bound` from the table. Loads the `apply_state` sidecar `provision_branch` wrote; per repo, before staging, verifies (design spec §3 item 5): the apply branch is still checked out, `HEAD` still equals the provisioned base (nothing self-committed during the caller's `pen exec` run), and every dirty path (`git status --porcelain`) is within the requested `paths` — any dirty path outside them fails the commit closed (`dirty-outside-bounds`), never `add -A`. Stages only the requested paths, commits, updates the sidecar with the resulting SHA.

- [ ] **Step 1: Write failing tests**

```rust
#[tokio::test]
async fn commit_stages_only_bound_paths() {
    let fx = nave_test_support::init_pen_fixture("commit-fx", "acme", "docs", "develop").await;
    let branch_req = nave_apply::BranchEnvelope {
        protocol_version: nave_apply::PROTOCOL_VERSION,
        repos: vec![nave_apply::BranchRepoRequest {
            repo: "acme/docs".into(), base_ref: "develop".into(),
            expected_base_sha: fx.base_sha.clone(), apply_ref: "pulse/apply/c1".into(),
        }],
    };
    provision_branch(fx.pen_root.path(), &fx.pen, &branch_req).await.unwrap();

    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "commit-fx", "acme", "docs");
    std::fs::write(dir.join("lockfile.json"), "{}").unwrap();

    let commit_req = nave_apply::CommitEnvelope {
        protocol_version: nave_apply::PROTOCOL_VERSION, message: "bump lockfile".into(),
        repos: vec![nave_apply::CommitRepoRequest { repo: "acme/docs".into(), paths: vec!["lockfile.json".into()] }],
    };
    let res = commit_bound(fx.pen_root.path(), &fx.pen, "pulse/apply/c1", &commit_req).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::CommitState::Ok));
    assert!(res.repos[0].local_commit_sha.is_some());
}

#[tokio::test]
async fn commit_fails_closed_on_dirty_path_outside_bounds() {
    let fx = nave_test_support::init_pen_fixture("commit-fx2", "acme", "docs", "develop").await;
    let branch_req = nave_apply::BranchEnvelope {
        protocol_version: nave_apply::PROTOCOL_VERSION,
        repos: vec![nave_apply::BranchRepoRequest {
            repo: "acme/docs".into(), base_ref: "develop".into(),
            expected_base_sha: fx.base_sha.clone(), apply_ref: "pulse/apply/c2".into(),
        }],
    };
    provision_branch(fx.pen_root.path(), &fx.pen, &branch_req).await.unwrap();

    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "commit-fx2", "acme", "docs");
    std::fs::write(dir.join("lockfile.json"), "{}").unwrap();
    std::fs::write(dir.join("unexpected.txt"), "surprise").unwrap();

    let commit_req = nave_apply::CommitEnvelope {
        protocol_version: nave_apply::PROTOCOL_VERSION, message: "bump lockfile".into(),
        repos: vec![nave_apply::CommitRepoRequest { repo: "acme/docs".into(), paths: vec!["lockfile.json".into()] }],
    };
    let res = commit_bound(fx.pen_root.path(), &fx.pen, "pulse/apply/c2", &commit_req).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::CommitState::DirtyOutsideBounds));
    let status = crate::git_util::git_output(&dir, &["status", "--porcelain"]).await.unwrap();
    assert!(!status.is_empty(), "nothing should have been committed");
}

#[tokio::test]
async fn commit_fails_closed_when_no_apply_state_recorded() {
    let fx = nave_test_support::init_pen_fixture("commit-fx3", "acme", "docs", "develop").await;
    let commit_req = nave_apply::CommitEnvelope {
        protocol_version: nave_apply::PROTOCOL_VERSION, message: "m".into(),
        repos: vec![nave_apply::CommitRepoRequest { repo: "acme/docs".into(), paths: vec!["x".into()] }],
    };
    let res = commit_bound(fx.pen_root.path(), &fx.pen, "pulse/apply/never-provisioned", &commit_req).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::CommitState::NoApplyState));
}
```

- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement.**

```rust
// crates/nave_pen/src/apply_ops.rs — append
use crate::apply_state::read_apply_state;

pub async fn commit_bound(
    pen_root: &Path,
    pen: &Pen,
    apply_ref: &str,
    request: &nave_apply::CommitEnvelope,
) -> anyhow::Result<nave_apply::CommitResult> {
    let repo_ids: Vec<String> = request.repos.iter().map(|r| r.repo.clone()).collect();
    if let Err(e) = nave_apply::validate_envelope_repos(request.protocol_version, &repo_ids) {
        return Ok(nave_apply::CommitResult {
            protocol_version: nave_apply::PROTOCOL_VERSION, adapter_state: nave_apply::AdapterState::Error,
            repos: vec![nave_apply::CommitRepoResult { repo: String::new(), local_commit_sha: None, state: nave_apply::CommitState::NoApplyState, reason: Some(e.to_string()) }],
        });
    }

    let mut state = read_apply_state(pen_root, &pen.name, apply_ref)?;
    let mut results = Vec::with_capacity(request.repos.len());
    for req in &request.repos {
        let (owner, name) = split_repo(&req.repo);
        let dir = pen_repo_clone_dir(pen_root, &pen.name, owner, name);
        let Some(repo_state) = state.repos.get(&req.repo) else {
            results.push(nave_apply::CommitRepoResult { repo: req.repo.clone(), local_commit_sha: None, state: nave_apply::CommitState::NoApplyState, reason: Some("no provisioned base recorded for this apply branch".into()) });
            continue;
        };
        let expected_base_sha = repo_state.expected_base_sha.clone();
        let result = commit_one(&dir, req, apply_ref, &expected_base_sha, &request.message).await;
        if let nave_apply::CommitState::Ok = result.state {
            if let Some(sha) = &result.local_commit_sha {
                state.repos.get_mut(&req.repo).unwrap().local_commit_sha = Some(sha.clone());
            }
        }
        results.push(result);
    }
    write_apply_state(pen_root, &pen.name, apply_ref, &state)?;

    Ok(nave_apply::CommitResult { protocol_version: nave_apply::PROTOCOL_VERSION, adapter_state: nave_apply::AdapterState::Ok, repos: results })
}

async fn commit_one(dir: &Path, req: &nave_apply::CommitRepoRequest, apply_ref: &str, expected_base_sha: &str, message: &str) -> nave_apply::CommitRepoResult {
    let mk = |state, sha: Option<String>, reason: Option<&str>| nave_apply::CommitRepoResult { repo: req.repo.clone(), local_commit_sha: sha, state, reason: reason.map(str::to_string) };
    if !dir.exists() {
        return mk(nave_apply::CommitState::MissingClone, None, Some("clone directory does not exist"));
    }
    let branch = match git_output(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await { Ok(b) => b, Err(e) => return mk(nave_apply::CommitState::InvariantViolated, None, Some(&e.to_string())) };
    if branch != apply_ref {
        return mk(nave_apply::CommitState::InvariantViolated, None, Some("checked-out branch changed since provisioning"));
    }
    let head = match git_output(dir, &["rev-parse", "HEAD"]).await { Ok(h) => h, Err(e) => return mk(nave_apply::CommitState::InvariantViolated, None, Some(&e.to_string())) };
    if head != expected_base_sha {
        return mk(nave_apply::CommitState::InvariantViolated, None, Some("HEAD moved since provisioning — unexpected commit during exec"));
    }
    let porcelain = match git_output(dir, &["status", "--porcelain"]).await { Ok(p) => p, Err(e) => return mk(nave_apply::CommitState::InvariantViolated, None, Some(&e.to_string())) };
    let dirty: Vec<&str> = porcelain.lines().map(|l| l[3..].trim()).collect();
    let bound: std::collections::HashSet<&str> = req.paths.iter().map(String::as_str).collect();
    if let Some(extra) = dirty.iter().find(|p| !bound.contains(*p)) {
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
    if let Err(e) = git_status(dir, &["commit", "-m", message]).await {
        return mk(nave_apply::CommitState::InvariantViolated, None, Some(&e.to_string()));
    }
    match git_output(dir, &["rev-parse", "HEAD"]).await {
        Ok(sha) => mk(nave_apply::CommitState::Ok, Some(sha), None),
        Err(e) => mk(nave_apply::CommitState::InvariantViolated, None, Some(&e.to_string())),
    }
}
```

CLI wiring mirrors Task 6's `PenBranchArgs`/`run_branch` shape with an added positional `branch` (the `apply_ref`) and `-m/--message`:

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
    let mut request: nave_apply::CommitEnvelope = serde_json::from_str(&raw).context("parsing request")?;
    request.message = args.message;
    let result = nave_pen::apply_ops::commit_bound(&root, &pen, &args.branch, &request).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    Ok(())
}
```

(The request file need not carry `message` at all in practice — the `-m` flag is authoritative; `CommitEnvelope.message` exists so the JSON contract stays self-describing when read back out of a written request file for logging.)

- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit** — `feat: nave pen commit verb (bounded staging + post-exec invariants)`.

---

### Task 8: `nave pen push` verb — structured results + exact coverage

**Files:** Modify `crates/nave_pen/src/apply_ops.rs`; Modify `crates/nave/src/commands/pen.rs` (`PenAction::Push`, `PenPushArgs`, `run_push`).

**Interfaces:** `apply_ops::push_branch` from the table. Reads the sidecar's recorded `local_commit_sha` per repo (written by `commit_bound`); before pushing, verifies local `HEAD` still equals it (`diverged` if not — catches a caller invoking `push` against a repo whose commit result it never received/persisted correctly); pushes with `--set-upstream origin <apply_ref>` (idempotent — re-pushing identical history fast-forwards to a no-op); reports `remote` (`git remote get-url origin`), `remote_ref` (the short `apply_ref`, matching what the pulse-gh driver compares against per spec §5B — never `refs/heads/...`), `remote_sha` (`git rev-parse origin/<apply_ref>`, valid immediately after a successful push since git updates the local remote-tracking ref), and `upstream` (`git rev-parse --abbrev-ref --symbolic-full-name @{upstream}`).

- [ ] **Step 1: Write failing tests**

```rust
#[tokio::test]
async fn push_reports_remote_sha_matching_local_commit() {
    let fx = nave_test_support::init_pen_fixture("push-fx", "acme", "docs", "develop").await;
    let branch_req = nave_apply::BranchEnvelope {
        protocol_version: nave_apply::PROTOCOL_VERSION,
        repos: vec![nave_apply::BranchRepoRequest { repo: "acme/docs".into(), base_ref: "develop".into(), expected_base_sha: fx.base_sha.clone(), apply_ref: "pulse/apply/pu1".into() }],
    };
    provision_branch(fx.pen_root.path(), &fx.pen, &branch_req).await.unwrap();
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "push-fx", "acme", "docs");
    std::fs::write(dir.join("lockfile.json"), "{}").unwrap();
    let commit_req = nave_apply::CommitEnvelope { protocol_version: nave_apply::PROTOCOL_VERSION, message: "m".into(), repos: vec![nave_apply::CommitRepoRequest { repo: "acme/docs".into(), paths: vec!["lockfile.json".into()] }] };
    let commit_res = commit_bound(fx.pen_root.path(), &fx.pen, "pulse/apply/pu1", &commit_req).await.unwrap();
    let local_sha = commit_res.repos[0].local_commit_sha.clone().unwrap();

    let push_req = nave_apply::PushEnvelope { protocol_version: nave_apply::PROTOCOL_VERSION, repos: vec![nave_apply::PushRepoRequest { repo: "acme/docs".into() }] };
    let res = push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/pu1", &push_req).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::PushState::Ok));
    assert_eq!(res.repos[0].remote_sha.as_deref(), Some(local_sha.as_str()));
    assert_eq!(res.repos[0].remote_ref.as_deref(), Some("pulse/apply/pu1"));
}

#[tokio::test]
async fn push_fails_closed_without_a_prior_commit() {
    let fx = nave_test_support::init_pen_fixture("push-fx2", "acme", "docs", "develop").await;
    let branch_req = nave_apply::BranchEnvelope { protocol_version: nave_apply::PROTOCOL_VERSION, repos: vec![nave_apply::BranchRepoRequest { repo: "acme/docs".into(), base_ref: "develop".into(), expected_base_sha: fx.base_sha.clone(), apply_ref: "pulse/apply/pu2".into() }] };
    provision_branch(fx.pen_root.path(), &fx.pen, &branch_req).await.unwrap();

    let push_req = nave_apply::PushEnvelope { protocol_version: nave_apply::PROTOCOL_VERSION, repos: vec![nave_apply::PushRepoRequest { repo: "acme/docs".into() }] };
    let res = push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/pu2", &push_req).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::PushState::NoApplyState));
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
) -> anyhow::Result<nave_apply::PushResult> {
    let repo_ids: Vec<String> = request.repos.iter().map(|r| r.repo.clone()).collect();
    if let Err(e) = nave_apply::validate_envelope_repos(request.protocol_version, &repo_ids) {
        return Ok(nave_apply::PushResult {
            protocol_version: nave_apply::PROTOCOL_VERSION, adapter_state: nave_apply::AdapterState::Error,
            repos: vec![nave_apply::PushRepoResult { repo: String::new(), remote: None, remote_ref: None, remote_sha: None, upstream: None, local_commit_sha: None, state: nave_apply::PushState::NoApplyState, reason: Some(e.to_string()) }],
        });
    }
    let state = read_apply_state(pen_root, &pen.name, apply_ref)?;
    let mut results = Vec::with_capacity(request.repos.len());
    for req in &request.repos {
        let (owner, name) = split_repo(&req.repo);
        let dir = pen_repo_clone_dir(pen_root, &pen.name, owner, name);
        let local_sha = state.repos.get(&req.repo).and_then(|r| r.local_commit_sha.clone());
        results.push(push_one(&dir, &req.repo, apply_ref, local_sha).await);
    }
    Ok(nave_apply::PushResult { protocol_version: nave_apply::PROTOCOL_VERSION, adapter_state: nave_apply::AdapterState::Ok, repos: results })
}

async fn push_one(dir: &Path, repo: &str, apply_ref: &str, expected_local_sha: Option<String>) -> nave_apply::PushRepoResult {
    let mk = |state, remote, remote_ref, remote_sha, upstream, local_commit_sha, reason: Option<&str>| nave_apply::PushRepoResult {
        repo: repo.to_string(), remote, remote_ref, remote_sha, upstream, local_commit_sha, state, reason: reason.map(str::to_string),
    };
    let Some(expected_local_sha) = expected_local_sha else {
        return mk(nave_apply::PushState::NoApplyState, None, None, None, None, None, Some("no committed local sha recorded for this repo"));
    };
    if !dir.exists() {
        return mk(nave_apply::PushState::MissingBranch, None, None, None, None, None, Some("clone directory does not exist"));
    }
    let head = match git_output(dir, &["rev-parse", "HEAD"]).await { Ok(h) => h, Err(e) => return mk(nave_apply::PushState::MissingBranch, None, None, None, None, None, Some(&e.to_string())) };
    if head != expected_local_sha {
        return mk(nave_apply::PushState::Diverged, None, None, None, None, Some(head), Some("local HEAD does not match the recorded commit"));
    }
    if let Err(e) = git_status(dir, &["push", "--set-upstream", "origin", apply_ref]).await {
        return mk(nave_apply::PushState::PushRejected, None, None, None, None, Some(head), Some(&e.to_string()));
    }
    let remote = git_output(dir, &["remote", "get-url", "origin"]).await.ok();
    let remote_sha = git_output(dir, &["rev-parse", &format!("origin/{apply_ref}")]).await.ok();
    let upstream = git_output(dir, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]).await.ok();
    mk(nave_apply::PushState::Ok, remote, Some(apply_ref.to_string()), remote_sha, upstream, Some(head), None)
}
```

CLI wiring mirrors `PenBranchArgs` with a `branch` positional, same as Task 7's commit CLI.

- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit** — `feat: nave pen push verb (structured results, exact coverage)`.

---

### Task 9: `nave pen reset` verb — CAS-guarded cleanup

**Files:** Modify `crates/nave_pen/src/apply_ops.rs`; Modify `crates/nave/src/commands/pen.rs` (`PenAction::Reset`, `PenResetArgs`, `run_reset`).

**Interfaces:** `apply_ops::reset_branch` from the table. Always resets the local clone off the apply branch back onto `pen.branch` and deletes the local `apply_ref` (idempotent — succeeds even if the branch never existed, reported via `local_reset: false`). The remote ref is deleted **only** if `expected_pushed_sha` is `Some` **and** `git ls-remote origin <apply_ref>` still reports that exact SHA — compare-and-swap, so cleanup never deletes a remote branch someone else has since force-pushed a replacement onto (design spec §3 item 4). Clears the `apply_state` sidecar on completion regardless of outcome (a failed reset still ends the apply attempt from Nave's point of view — retrying is a fresh `branch` call).

- [ ] **Step 1: Write failing tests**

```rust
#[tokio::test]
async fn reset_deletes_remote_ref_only_on_sha_match() {
    let fx = nave_test_support::init_pen_fixture("reset-fx", "acme", "docs", "develop").await;
    let branch_req = nave_apply::BranchEnvelope { protocol_version: nave_apply::PROTOCOL_VERSION, repos: vec![nave_apply::BranchRepoRequest { repo: "acme/docs".into(), base_ref: "develop".into(), expected_base_sha: fx.base_sha.clone(), apply_ref: "pulse/apply/r1".into() }] };
    provision_branch(fx.pen_root.path(), &fx.pen, &branch_req).await.unwrap();
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "reset-fx", "acme", "docs");
    std::fs::write(dir.join("lockfile.json"), "{}").unwrap();
    let commit_req = nave_apply::CommitEnvelope { protocol_version: nave_apply::PROTOCOL_VERSION, message: "m".into(), repos: vec![nave_apply::CommitRepoRequest { repo: "acme/docs".into(), paths: vec!["lockfile.json".into()] }] };
    let commit_res = commit_bound(fx.pen_root.path(), &fx.pen, "pulse/apply/r1", &commit_req).await.unwrap();
    let local_sha = commit_res.repos[0].local_commit_sha.clone().unwrap();
    push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/r1", &nave_apply::PushEnvelope { protocol_version: nave_apply::PROTOCOL_VERSION, repos: vec![nave_apply::PushRepoRequest { repo: "acme/docs".into() }] }).await.unwrap();

    let reset_req = nave_apply::ResetEnvelope { protocol_version: nave_apply::PROTOCOL_VERSION, repos: vec![nave_apply::ResetRepoRequest { repo: "acme/docs".into(), expected_pushed_sha: Some(local_sha) }] };
    let res = reset_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/r1", &reset_req).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::ResetState::Ok));
    assert!(res.repos[0].local_reset);
    assert!(res.repos[0].remote_deleted);

    let remote_refs = crate::git_util::git_output(&fx.origin.path(), &["for-each-ref", "refs/heads/pulse/apply/r1"]).await.unwrap();
    assert!(remote_refs.is_empty());
}

#[tokio::test]
async fn reset_skips_remote_delete_on_cas_mismatch() {
    let fx = nave_test_support::init_pen_fixture("reset-fx2", "acme", "docs", "develop").await;
    let branch_req = nave_apply::BranchEnvelope { protocol_version: nave_apply::PROTOCOL_VERSION, repos: vec![nave_apply::BranchRepoRequest { repo: "acme/docs".into(), base_ref: "develop".into(), expected_base_sha: fx.base_sha.clone(), apply_ref: "pulse/apply/r2".into() }] };
    provision_branch(fx.pen_root.path(), &fx.pen, &branch_req).await.unwrap();
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "reset-fx2", "acme", "docs");
    std::fs::write(dir.join("lockfile.json"), "{}").unwrap();
    let commit_req = nave_apply::CommitEnvelope { protocol_version: nave_apply::PROTOCOL_VERSION, message: "m".into(), repos: vec![nave_apply::CommitRepoRequest { repo: "acme/docs".into(), paths: vec!["lockfile.json".into()] }] };
    commit_bound(fx.pen_root.path(), &fx.pen, "pulse/apply/r2", &commit_req).await.unwrap();
    push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/r2", &nave_apply::PushEnvelope { protocol_version: nave_apply::PROTOCOL_VERSION, repos: vec![nave_apply::PushRepoRequest { repo: "acme/docs".into() }] }).await.unwrap();

    let reset_req = nave_apply::ResetEnvelope { protocol_version: nave_apply::PROTOCOL_VERSION, repos: vec![nave_apply::ResetRepoRequest { repo: "acme/docs".into(), expected_pushed_sha: Some("f".repeat(40)) }] };
    let res = reset_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/r2", &reset_req).await.unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::ResetState::RemoteCasMismatch));
    assert!(!res.repos[0].remote_deleted);
    let remote_refs = crate::git_util::git_output(&fx.origin.path(), &["for-each-ref", "refs/heads/pulse/apply/r2"]).await.unwrap();
    assert!(!remote_refs.is_empty(), "remote branch must survive a CAS mismatch");
}
```

- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement.**

```rust
// crates/nave_pen/src/apply_ops.rs — append
use crate::apply_state::clear_apply_state;

pub async fn reset_branch(
    pen_root: &Path,
    pen: &Pen,
    apply_ref: &str,
    request: &nave_apply::ResetEnvelope,
) -> anyhow::Result<nave_apply::ResetResult> {
    let repo_ids: Vec<String> = request.repos.iter().map(|r| r.repo.clone()).collect();
    if let Err(e) = nave_apply::validate_envelope_repos(request.protocol_version, &repo_ids) {
        return Ok(nave_apply::ResetResult {
            protocol_version: nave_apply::PROTOCOL_VERSION, adapter_state: nave_apply::AdapterState::Error,
            repos: vec![nave_apply::ResetRepoResult { repo: String::new(), local_reset: false, remote_deleted: false, state: nave_apply::ResetState::MissingBranch, reason: Some(e.to_string()) }],
        });
    }
    let mut results = Vec::with_capacity(request.repos.len());
    for req in &request.repos {
        let (owner, name) = split_repo(&req.repo);
        let dir = pen_repo_clone_dir(pen_root, &pen.name, owner, name);
        results.push(reset_one(&dir, &pen.branch, apply_ref, req).await);
    }
    clear_apply_state(pen_root, &pen.name, apply_ref)?;
    Ok(nave_apply::ResetResult { protocol_version: nave_apply::PROTOCOL_VERSION, adapter_state: nave_apply::AdapterState::Ok, repos: results })
}

async fn reset_one(dir: &Path, pen_branch: &str, apply_ref: &str, req: &nave_apply::ResetRepoRequest) -> nave_apply::ResetRepoResult {
    let mut local_reset = false;
    let mut state = nave_apply::ResetState::Ok;
    let mut reason: Option<String> = None;

    if dir.exists() && git_ok(dir, &["rev-parse", "--verify", "--quiet", apply_ref]).await.unwrap_or(false) {
        let on_apply_ref = git_output(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await.ok().as_deref() == Some(apply_ref);
        if on_apply_ref {
            let _ = git_status(dir, &["checkout", pen_branch]).await;
        }
        local_reset = git_status(dir, &["branch", "-D", apply_ref]).await.is_ok();
        if !local_reset {
            state = nave_apply::ResetState::MissingBranch;
            reason = Some("failed to delete local apply branch".into());
        }
    }

    let mut remote_deleted = false;
    if let Some(expected) = &req.expected_pushed_sha {
        let remote_line = git_output(dir, &["ls-remote", "origin", apply_ref]).await.unwrap_or_default();
        let observed_remote_sha = remote_line.split_whitespace().next().unwrap_or("");
        if observed_remote_sha.is_empty() {
            // already gone remotely — idempotent no-op, not a failure
        } else if observed_remote_sha == expected {
            remote_deleted = git_status(dir, &["push", "origin", "--delete", apply_ref]).await.is_ok();
            if !remote_deleted {
                state = nave_apply::ResetState::RemoteCasMismatch;
                reason = Some("remote delete failed after CAS match".into());
            }
        } else {
            state = nave_apply::ResetState::RemoteCasMismatch;
            reason = Some(format!("remote sha {observed_remote_sha} does not match expected {expected} — leaving remote branch intact"));
        }
    }

    nave_apply::ResetRepoResult { repo: req.repo.clone(), local_reset, remote_deleted, state, reason }
}
```

CLI wiring mirrors Task 8's `push` command shape (`name`, `branch` positional, `--request`, `--json`).

- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit** — `feat: nave pen reset verb (CAS-guarded cleanup)`.

---

### Task 10: End-to-end integration proof (real clones, real remote)

**Files:** Create `crates/nave/tests/pen_apply.rs`; Modify `crates/nave/Cargo.toml` (`[dev-dependencies]`: `nave_test_support`, `nave_apply`, `nave_pen`).

**Interfaces:** drives the **real CLI binary** (`CARGO_BIN_EXE_nave`, matching `smoke.rs`'s existing pattern) through `capabilities → branch → commit → push → reset`, writing real request files to a tempdir and parsing real stdout JSON — this is the proof design spec §7 requires ("the 'output actually lands on real clones' proof lives in the Nave fork's suite").

- [ ] **Step 1: Write failing test**

```rust
// crates/nave/tests/pen_apply.rs
use std::process::Command;

fn nave(args: &[&str], home: &std::path::Path) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_nave")).args(args).env("HOME", home).output().unwrap();
    assert!(out.status.success(), "nave {args:?} exited {:?}: {}", out.status, String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("bad json from nave {args:?}: {e}: {}", String::from_utf8_lossy(&out.stdout)))
}

#[tokio::test]
async fn full_apply_lifecycle_lands_on_real_clone_and_cleans_up() {
    let fx = nave_test_support::init_pen_fixture("e2e-apply", "acme", "docs", "develop").await;
    let home = tempfile::TempDir::new().unwrap();
    // nave pen capabilities --json
    let caps = nave(&["pen", "capabilities", "--json"], home.path());
    assert_eq!(caps["verbs"].as_array().unwrap().len(), 4);

    let reqdir = tempfile::TempDir::new().unwrap();
    let branch_req = reqdir.path().join("branch.json");
    std::fs::write(&branch_req, format!(
        r#"{{"protocol_version":1,"repos":[{{"repo":"acme/docs","base_ref":"develop","expected_base_sha":"{}","apply_ref":"pulse/apply/e2e"}}]}}"#,
        fx.base_sha,
    )).unwrap();
    let branch_res = nave(&["pen", "branch", "e2e-apply", "--request", branch_req.to_str().unwrap(), "--json"], home.path());
    assert_eq!(branch_res["repos"][0]["state"], "ok");

    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "e2e-apply", "acme", "docs");
    std::fs::write(dir.join("lockfile.json"), "{}").unwrap();

    let commit_req = reqdir.path().join("commit.json");
    std::fs::write(&commit_req, r#"{"protocol_version":1,"repos":[{"repo":"acme/docs","paths":["lockfile.json"]}]}"#).unwrap();
    let commit_res = nave(&["pen", "commit", "e2e-apply", "pulse/apply/e2e", "--request", commit_req.to_str().unwrap(), "-m", "bump lockfile", "--json"], home.path());
    assert_eq!(commit_res["repos"][0]["state"], "ok");
    let local_sha = commit_res["repos"][0]["local_commit_sha"].as_str().unwrap().to_string();

    let push_req = reqdir.path().join("push.json");
    std::fs::write(&push_req, r#"{"protocol_version":1,"repos":[{"repo":"acme/docs"}]}"#).unwrap();
    let push_res = nave(&["pen", "push", "e2e-apply", "pulse/apply/e2e", "--request", push_req.to_str().unwrap(), "--json"], home.path());
    assert_eq!(push_res["repos"][0]["state"], "ok");
    assert_eq!(push_res["repos"][0]["remote_sha"], local_sha);

    let reset_req = reqdir.path().join("reset.json");
    std::fs::write(&reset_req, format!(r#"{{"protocol_version":1,"repos":[{{"repo":"acme/docs","expected_pushed_sha":"{local_sha}"}}]}}"#)).unwrap();
    let reset_res = nave(&["pen", "reset", "e2e-apply", "pulse/apply/e2e", "--request", reset_req.to_str().unwrap(), "--json"], home.path());
    assert_eq!(reset_res["repos"][0]["state"], "ok");
    assert_eq!(reset_res["repos"][0]["remote_deleted"], true);
}
```

(This requires wiring the CLI's `resolve_pen_root`/pen storage layer to the fixture's `pen_root` — `HOME`-scoped config alone does not point `nave` at `fx.pen_root`; the implementer must additionally write a `nave.toml` under `home/.config/nave.toml` setting `[pen] root = "<fx.pen_root>"` before Step 1's test can pass, following `smoke.rs`'s existing `write_config` helper as the template. This step is called out explicitly rather than left implicit — add a `write_pen_config(home, fx.pen_root.path())` helper alongside `nave()` in this file.)

- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** the `write_pen_config` helper (mirrors `smoke.rs`'s `write_config`) and confirm the CLI's `load_default()`/`resolve_pen_root()` path picks up the pointed-at root — no production code changes expected in this step if Tasks 1-9 wired the CLI correctly; this task's "implementation" is closing any gap that surfaces.
- [ ] **Step 4: Run, verify pass** (`cargo test -p nave --test pen_apply`).
- [ ] **Step 5: Commit** — `test: end-to-end apply-verb lifecycle against real clones`.

---

### Task 11: Docs

**Files:** Modify `docs/reference/cli/pen.md` (overview — add the five verbs to the command list); Replace stub content in `docs/reference/cli/pen/branch.md` (create — no existing stub with this name), `docs/reference/cli/pen/commit.md` (create), `docs/reference/cli/pen/push.md` (create — distinct from `pen exec --push-changes`, unrelated), `docs/reference/cli/pen/reset.md` (create), `docs/reference/cli/pen/capabilities.md` (create); Modify `.just/docs_nav.just` if it hand-lists pages (check; if nav is directory-driven, no change needed).

- [ ] **Step 1:** For each new verb, write a real reference page following the existing `docs/reference/cli/pen/status.md`/`exec.md` structure (command synopsis, flags table, JSON shape, one example invocation + example output). No stub placeholders.
- [ ] **Step 2:** Update `docs/reference/cli/pen.md`'s command list to include the five new subcommands with one-line descriptions matching the `PenAction` doc comments from Tasks 5-9.
- [ ] **Step 3:** `just check-docs` (or `just build-docs` if `check-docs` requires zensical network access unavailable in this environment — verify which and record the actual command that ran clean) and **commit** — `docs: reference pages for the five apply verbs`.

---

### Task 12: Coordination handoff note for `hiivmind-pulse-gh`

**Files:** Create `docs/superpowers/specs/2026-08-13-apply-verb-contract-handoff.md`.

- [ ] **Step 1:** Write a short note (not a full spec) recording: the final JSON wire shape for all five verbs as shipped (copy the `nave_apply` types verbatim, post-implementation, in case any detail shifted during Task 3's review); the three coordination points already listed in this plan's "Coordination with hiivmind-pulse-gh" section; the CLI invocation shapes (`nave pen branch <name> --request <file> --json`, etc.) the Python `nave_adapter.py` argv-builders must match exactly; and an explicit instruction that `hiivmind-pulse-gh`'s Task 1 implementer should read this file, not re-derive the contract from the (now superseded-in-detail) Authoritative Interfaces table alone.
- [ ] **Step 2: Commit** — `docs: apply-verb contract handoff note for hiivmind-pulse-gh Task 1`.

---

## Completion note

A green suite here proves the five verbs work against real, local git remotes — the "Rust half" of F11 production wiring (design spec §8 step 1 of 3). It does **not** land apply-mode end-to-end: `hiivmind-pulse-gh`'s own 12-task plan (Task 1 onward) still has to consume this contract, delete its raw-git trio, and build the driver; `hiivmind/hiivmind-workspace` enrollment is last. Do not report this plan's completion as "F11 shipped."

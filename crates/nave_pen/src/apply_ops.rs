//! Apply-mode mutation verbs: branch/commit/push/reset over a Nave pen,
//! plus the stateless capabilities probe. Mirrors `ops.rs`'s mutation
//! idiom; uses `git_util` (captured output) and `apply_state` (the
//! cross-invocation sidecar) instead of `ops.rs`'s fire-and-forget helpers.

use nave_apply::{APPLY_VERBS, AdapterState, CapabilitiesResult, PROTOCOL_VERSION};

pub fn capabilities() -> CapabilitiesResult {
    CapabilitiesResult {
        protocol_version: PROTOCOL_VERSION,
        verbs: APPLY_VERBS.iter().map(ToString::to_string).collect(),
        adapter_state: AdapterState::Ok,
        reason: None,
    }
}

use std::path::Path;

use anyhow::Result as AResult;

use crate::apply_state::{ApplyRepoState, clear_apply_state, read_apply_state, write_apply_state};
use crate::git_util::{git_ok, git_output, git_output_raw, git_status};
use crate::storage::{Pen, PenRepo, pen_repo_clone_dir};

pub(crate) fn resolve_repo<'a>(pen: &'a Pen, repo_id: &str) -> Option<&'a PenRepo> {
    let (owner, name) = repo_id.split_once('/')?;
    pen.repos
        .iter()
        .find(|r| r.owner == owner && r.name == name)
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
                repo: req.repo.clone(),
                base_ref: req.base_ref.clone(),
                expected_base_sha: req.expected_base_sha.clone(),
                observed_base_sha: String::new(),
                apply_ref: request.apply_ref.clone(),
                state: nave_apply::BranchState::UnknownRepo,
                reason: Some("repo is not part of this pen".into()),
            },
            Some(pen_repo) => {
                let dir = pen_repo_clone_dir(pen_root, &pen.name, &pen_repo.owner, &pen_repo.name);
                let mut result = provision_one(&dir, req, &request.apply_ref).await;
                if matches!(result.state, nave_apply::BranchState::Ok) {
                    // Both reads must succeed or provisioning does not report `ok` — a
                    // half-captured sidecar would silently break `commit`/`push`'s later
                    // origin-integrity checks.
                    let fetch_url = git_output(&dir, &["remote", "get-url", "origin"]).await;
                    let push_url =
                        git_output(&dir, &["remote", "get-url", "--push", "origin"]).await;
                    if let (Ok(fetch_url), Ok(push_url)) = (fetch_url, push_url) {
                        let mut state = read_apply_state(pen_root, &pen.name, &request.apply_ref)?;
                        state.repos.insert(
                            req.repo.clone(),
                            ApplyRepoState {
                                base_ref: req.base_ref.clone(),
                                expected_base_sha: req.expected_base_sha.clone(),
                                expected_origin_url: fetch_url,
                                expected_push_url: push_url,
                                local_commit_sha: None,
                            },
                        );
                        write_apply_state(pen_root, &pen.name, &request.apply_ref, &state)?;
                    } else {
                        result.state = nave_apply::BranchState::EvidenceUnavailable;
                        result.reason =
                            Some("checked out but could not capture origin remote urls".into());
                    }
                }
                result
            }
        };
        results.push(result);
    }

    Ok(nave_apply::BranchResult {
        protocol_version: nave_apply::PROTOCOL_VERSION,
        adapter_state: nave_apply::AdapterState::Ok,
        reason: None,
        repos: results,
    })
}

async fn provision_one(
    dir: &Path,
    req: &nave_apply::BranchRepoRequest,
    apply_ref: &str,
) -> nave_apply::BranchRepoResult {
    let mk = |state, observed: String, reason: Option<&str>| nave_apply::BranchRepoResult {
        repo: req.repo.clone(),
        base_ref: req.base_ref.clone(),
        expected_base_sha: req.expected_base_sha.clone(),
        observed_base_sha: observed,
        apply_ref: apply_ref.to_string(),
        state,
        reason: reason.map(str::to_string),
    };
    if !dir.exists() {
        return mk(
            nave_apply::BranchState::MissingRef,
            String::new(),
            Some("clone directory does not exist"),
        );
    }
    if let Err(e) = git_status(dir, &["fetch", "--depth=1", "origin", &req.base_ref]).await {
        return mk(
            nave_apply::BranchState::MissingRef,
            String::new(),
            Some(&e.to_string()),
        );
    }
    let observed = match git_output(dir, &["rev-parse", &format!("origin/{}", req.base_ref)]).await
    {
        Ok(sha) => sha,
        Err(e) => {
            return mk(
                nave_apply::BranchState::MissingRef,
                String::new(),
                Some(&e.to_string()),
            );
        }
    };
    if git_output(dir, &["cat-file", "-t", &observed])
        .await
        .ok()
        .as_deref()
        != Some("commit")
    {
        return mk(
            nave_apply::BranchState::NotACommit,
            observed,
            Some("resolved object is not a commit"),
        );
    }
    if observed != req.expected_base_sha {
        return mk(
            nave_apply::BranchState::StaleBase,
            observed,
            Some("observed base sha does not match expected"),
        );
    }
    if git_ok(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{apply_ref}"),
        ],
    )
    .await
    .unwrap_or(false)
    {
        return mk(
            nave_apply::BranchState::Exists,
            observed,
            Some("apply branch already exists"),
        );
    }
    if let Err(e) = git_status(dir, &["checkout", "-B", apply_ref, &observed]).await {
        return mk(
            nave_apply::BranchState::NotACommit,
            observed,
            Some(&e.to_string()),
        );
    }
    mk(nave_apply::BranchState::Ok, observed, None)
}

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
            results.push(nave_apply::CommitRepoResult {
                repo: req.repo.clone(),
                local_commit_sha: None,
                state: nave_apply::CommitState::UnknownRepo,
                reason: Some("repo is not part of this pen".into()),
            });
            continue;
        };
        let dir = pen_repo_clone_dir(pen_root, &pen.name, &pen_repo.owner, &pen_repo.name);
        let Some(repo_state) = state.repos.get(&req.repo).cloned() else {
            results.push(nave_apply::CommitRepoResult {
                repo: req.repo.clone(),
                local_commit_sha: None,
                state: nave_apply::CommitState::NoApplyState,
                reason: Some("no provisioned base recorded for this apply branch".into()),
            });
            continue;
        };
        let result = commit_one(&dir, req, apply_ref, &repo_state, message).await;
        if let (nave_apply::CommitState::Ok, Some(sha)) = (&result.state, &result.local_commit_sha)
        {
            state.repos.get_mut(&req.repo).unwrap().local_commit_sha = Some(sha.clone());
            write_apply_state(pen_root, &pen.name, apply_ref, &state)?;
        }
        results.push(result);
    }

    Ok(nave_apply::CommitResult {
        protocol_version: nave_apply::PROTOCOL_VERSION,
        adapter_state: nave_apply::AdapterState::Ok,
        reason: None,
        repos: results,
    })
}

fn dirty_paths_from_porcelain(porcelain: &str) -> Vec<String> {
    porcelain
        .lines()
        .filter(|l| l.len() > 3)
        .flat_map(|l| {
            let rest = l[3..].trim_matches('"');
            if let Some((old, new)) = rest.split_once(" -> ") {
                vec![old.to_string(), new.to_string()]
            } else {
                vec![rest.to_string()]
            }
        })
        .collect()
}

async fn verify_pre_commit_state(
    dir: &Path,
    apply_ref: &str,
    repo_state: &ApplyRepoState,
) -> Result<(), String> {
    let branch = git_output(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .map_err(|e| e.to_string())?;
    if branch != apply_ref {
        return Err("checked-out branch changed since provisioning".into());
    }
    let head = git_output(dir, &["rev-parse", "HEAD"])
        .await
        .map_err(|e| e.to_string())?;
    if head != repo_state.expected_base_sha {
        return Err("HEAD moved since provisioning — unexpected commit during exec".into());
    }
    let origin_url = git_output(dir, &["remote", "get-url", "origin"])
        .await
        .map_err(|e| e.to_string())?;
    if origin_url != repo_state.expected_origin_url {
        return Err("origin remote url changed since provisioning".into());
    }
    Ok(())
}

async fn read_dirty_paths(dir: &Path) -> Result<Vec<String>, String> {
    let porcelain = git_output_raw(dir, &["status", "--porcelain"])
        .await
        .map_err(|e| e.to_string())?;
    Ok(dirty_paths_from_porcelain(&porcelain))
}

async fn stage_and_commit(dir: &Path, paths: &[String], message: &str) -> Result<String, String> {
    for p in paths {
        git_status(dir, &["add", "--", p])
            .await
            .map_err(|_| format!("failed to stage {p}"))?;
    }
    // Hooks disabled for Nave's own commit: nothing an ecosystem command planted in
    // `.git/hooks` (pre-commit, commit-msg, etc.) fires during this call.
    git_status(
        dir,
        &["-c", "core.hooksPath=/dev/null", "commit", "-m", message],
    )
    .await
    .map_err(|e| e.to_string())?;
    git_output(dir, &["rev-parse", "HEAD"])
        .await
        .map_err(|e| e.to_string())
}

/// The committed tree must touch only the requested paths — a defense-in-depth check even
/// with hooks disabled and bounded staging, since it inspects the actual commit that landed.
async fn verify_post_commit_bounds(
    dir: &Path,
    base_sha: &str,
    sha: &str,
    bound: &std::collections::HashSet<&str>,
) -> Result<(), String> {
    let changed = git_output(dir, &["diff", "--name-only", base_sha, sha])
        .await
        .map_err(|e| e.to_string())?;
    if let Some(extra) = changed.lines().find(|p| !bound.contains(*p)) {
        return Err(format!(
            "committed tree touched {extra}, outside bound_paths"
        ));
    }
    Ok(())
}

async fn commit_one(
    dir: &Path,
    req: &nave_apply::CommitRepoRequest,
    apply_ref: &str,
    repo_state: &ApplyRepoState,
    message: &str,
) -> nave_apply::CommitRepoResult {
    let mk = |state, sha: Option<String>, reason: Option<&str>| nave_apply::CommitRepoResult {
        repo: req.repo.clone(),
        local_commit_sha: sha,
        state,
        reason: reason.map(str::to_string),
    };
    if !dir.exists() {
        return mk(
            nave_apply::CommitState::MissingClone,
            None,
            Some("clone directory does not exist"),
        );
    }
    if let Err(reason) = verify_pre_commit_state(dir, apply_ref, repo_state).await {
        return mk(
            nave_apply::CommitState::InvariantViolated,
            None,
            Some(&reason),
        );
    }

    let dirty = match read_dirty_paths(dir).await {
        Ok(d) => d,
        Err(reason) => {
            return mk(
                nave_apply::CommitState::InvariantViolated,
                None,
                Some(&reason),
            );
        }
    };
    let bound: std::collections::HashSet<&str> = req.paths.iter().map(String::as_str).collect();
    if let Some(extra) = dirty.iter().find(|p| !bound.contains(p.as_str())) {
        return mk(
            nave_apply::CommitState::DirtyOutsideBounds,
            None,
            Some(&format!("{extra} is dirty but not in bound_paths")),
        );
    }
    if dirty.is_empty() {
        return mk(nave_apply::CommitState::NothingToCommit, None, None);
    }

    let sha = match stage_and_commit(dir, &req.paths, message).await {
        Ok(s) => s,
        Err(reason) => {
            return mk(
                nave_apply::CommitState::InvariantViolated,
                None,
                Some(&reason),
            );
        }
    };
    if let Err(reason) =
        verify_post_commit_bounds(dir, &repo_state.expected_base_sha, &sha, &bound).await
    {
        return mk(
            nave_apply::CommitState::InvariantViolated,
            Some(sha),
            Some(&reason),
        );
    }
    mk(nave_apply::CommitState::Ok, Some(sha), None)
}

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
            results.push(nave_apply::PushRepoResult {
                repo: req.repo.clone(),
                remote: None,
                remote_ref: None,
                remote_sha: None,
                upstream: None,
                local_commit_sha: None,
                state: nave_apply::PushState::UnknownRepo,
                reason: Some("repo is not part of this pen".into()),
            });
            continue;
        };
        let dir = pen_repo_clone_dir(pen_root, &pen.name, &pen_repo.owner, &pen_repo.name);
        results.push(
            push_one(
                &dir,
                &req.repo,
                apply_ref,
                state.repos.get(&req.repo).cloned(),
            )
            .await,
        );
    }
    Ok(nave_apply::PushResult {
        protocol_version: nave_apply::PROTOCOL_VERSION,
        adapter_state: nave_apply::AdapterState::Ok,
        reason: None,
        repos: results,
    })
}

async fn verify_push_preconditions(
    dir: &Path,
    apply_ref: &str,
    repo_state: &ApplyRepoState,
) -> Result<String, (nave_apply::PushState, String)> {
    let branch_sha = git_output(dir, &["rev-parse", &format!("refs/heads/{apply_ref}")])
        .await
        .map_err(|e| (nave_apply::PushState::MissingBranch, e.to_string()))?;
    let Some(expected) = &repo_state.local_commit_sha else {
        return Err((
            nave_apply::PushState::NoApplyState,
            "no committed local sha recorded for this repo".into(),
        ));
    };
    if &branch_sha != expected {
        return Err((
            nave_apply::PushState::Diverged,
            "apply branch tip does not match the recorded commit".into(),
        ));
    }
    // Check the PUSH destination, not the fetch URL: `git push` uses `remote.origin.pushurl`
    // when configured, which can differ from `remote.origin.url` — an ecosystem command that
    // only changes the pushurl would pass a fetch-url-only check while still redirecting
    // where this push actually lands.
    let push_url = git_output(dir, &["remote", "get-url", "--push", "origin"])
        .await
        .map_err(|e| (nave_apply::PushState::PushRejected, e.to_string()))?;
    if push_url != repo_state.expected_push_url {
        return Err((
            nave_apply::PushState::PushRejected,
            "origin push url changed since provisioning".into(),
        ));
    }
    Ok(branch_sha)
}

struct PushEvidence {
    remote: String,
    remote_sha: String,
    upstream: Option<String>,
}

async fn perform_push(
    dir: &Path,
    apply_ref: &str,
    repo_state: &ApplyRepoState,
) -> Result<(String, PushEvidence), (nave_apply::PushState, Option<String>, String)> {
    let branch_sha = verify_push_preconditions(dir, apply_ref, repo_state)
        .await
        .map_err(|(state, reason)| (state, None, reason))?;
    // `-u`/`--set-upstream` in addition to the explicit refspec: the refspec makes the push
    // target unambiguous; `-u` makes upstream tracking reliable rather than incidental.
    if let Err(e) = git_status(
        dir,
        &[
            "push",
            "--set-upstream",
            "origin",
            &format!("refs/heads/{apply_ref}:refs/heads/{apply_ref}"),
        ],
    )
    .await
    {
        return Err((
            nave_apply::PushState::PushRejected,
            Some(branch_sha),
            e.to_string(),
        ));
    }
    let Ok(remote) = git_output(dir, &["remote", "get-url", "origin"]).await else {
        return Err((
            nave_apply::PushState::PushRejected,
            Some(branch_sha),
            "push succeeded but remote url could not be re-read".into(),
        ));
    };
    // Verify against the authoritative remote ref (ls-remote), never the
    // local remote-tracking ref: `create_pen` clones with `--depth=1`,
    // whose single-branch fetch refspec creates tracking refs only for the
    // default branch, so `rev-parse origin/<apply_ref>` fails after pushing
    // a NEW branch even though the push landed.
    let Ok(ls_remote) = git_output(dir, &["ls-remote", "origin", &format!("refs/heads/{apply_ref}")]).await
    else {
        return Err((
            nave_apply::PushState::PushRejected,
            Some(branch_sha),
            "push succeeded but remote sha could not be verified".into(),
        ));
    };
    let remote_sha = ls_remote.split_whitespace().next().unwrap_or("").to_string();
    if remote_sha.is_empty() {
        return Err((
            nave_apply::PushState::PushRejected,
            Some(branch_sha),
            "push succeeded but remote sha could not be verified".into(),
        ));
    };
    // The remote tip must be exactly what we just pushed — anything else means the push
    // landed somewhere unexpected (or something else raced onto the branch mid-push).
    if remote_sha != branch_sha {
        return Err((
            nave_apply::PushState::PushRejected,
            Some(branch_sha.clone()),
            format!("remote sha {remote_sha} does not match the pushed commit {branch_sha}"),
        ));
    }
    let upstream = git_output(
        dir,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .await
    .ok();
    Ok((
        branch_sha,
        PushEvidence {
            remote,
            remote_sha,
            upstream,
        },
    ))
}

async fn push_one(
    dir: &Path,
    repo: &str,
    apply_ref: &str,
    repo_state: Option<ApplyRepoState>,
) -> nave_apply::PushRepoResult {
    let base = nave_apply::PushRepoResult {
        repo: repo.to_string(),
        remote: None,
        remote_ref: None,
        remote_sha: None,
        upstream: None,
        local_commit_sha: None,
        state: nave_apply::PushState::Ok,
        reason: None,
    };
    let Some(repo_state) = repo_state else {
        return nave_apply::PushRepoResult {
            state: nave_apply::PushState::NoApplyState,
            reason: Some("no provisioned/committed state recorded for this repo".into()),
            ..base
        };
    };
    if !dir.exists() {
        return nave_apply::PushRepoResult {
            state: nave_apply::PushState::MissingBranch,
            reason: Some("clone directory does not exist".into()),
            ..base
        };
    }
    match perform_push(dir, apply_ref, &repo_state).await {
        Ok((branch_sha, ev)) => nave_apply::PushRepoResult {
            remote: Some(ev.remote),
            remote_ref: Some(apply_ref.to_string()),
            remote_sha: Some(ev.remote_sha),
            upstream: ev.upstream,
            local_commit_sha: Some(branch_sha),
            state: nave_apply::PushState::Ok,
            ..base
        },
        Err((state, local_commit_sha, reason)) => nave_apply::PushRepoResult {
            state,
            local_commit_sha,
            reason: Some(reason),
            ..base
        },
    }
}

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
        if let Some(sha) = &r.expected_pushed_sha
            && let Err(e) = nave_apply::validate_hex_sha(sha)
        {
            error_envelope!(ResetResult, e);
        }
    }

    let apply_state = read_apply_state(pen_root, &pen.name, apply_ref)?;
    let mut results = Vec::with_capacity(request.repos.len());
    for req in &request.repos {
        let result = match resolve_repo(pen, &req.repo) {
            None => nave_apply::ResetRepoResult {
                repo: req.repo.clone(),
                local_reset: false,
                remote_deleted: false,
                state: nave_apply::ResetState::UnknownRepo,
                reason: Some("repo is not part of this pen".into()),
            },
            Some(pen_repo) => {
                let dir = pen_repo_clone_dir(pen_root, &pen.name, &pen_repo.owner, &pen_repo.name);
                let repo_state = apply_state.repos.get(&req.repo);
                reset_one(&dir, &pen.branch, apply_ref, req, repo_state).await
            }
        };
        results.push(result);
    }
    clear_apply_state(pen_root, &pen.name, apply_ref)?;
    Ok(nave_apply::ResetResult {
        protocol_version: nave_apply::PROTOCOL_VERSION,
        adapter_state: nave_apply::AdapterState::Ok,
        reason: None,
        repos: results,
    })
}

/// Local cleanup: moves off the apply branch if it's currently checked out (`pen.branch`
/// always exists locally — every real pen clone is checked out onto it by `create_pen`'s
/// `clone_and_branch`, replicated by the test fixture), then deletes the apply branch. Every
/// step's failure — including the dirty-state discard itself — is propagated, never silently
/// swallowed: a `reset --hard`/`clean -fd` failure means the working tree may still hold
/// partial apply output, which must never be reported as a clean `local_reset: true`.
async fn reset_local(dir: &Path, pen_branch: &str, apply_ref: &str) -> Result<bool, String> {
    git_status(dir, &["reset", "--hard", "HEAD"])
        .await
        .map_err(|e| format!("failed to discard dirty state: {e}"))?;
    git_status(dir, &["clean", "-fd"])
        .await
        .map_err(|e| format!("failed to clean untracked files: {e}"))?;

    let current_branch = git_output(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .map_err(|e| e.to_string())?;
    if current_branch == apply_ref {
        git_status(dir, &["checkout", pen_branch])
            .await
            .map_err(|e| {
                format!("failed to check out {pen_branch} before deleting apply branch: {e}")
            })?;
    }

    let apply_branch_exists = git_ok(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{apply_ref}"),
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    if !apply_branch_exists {
        return Ok(true); // already gone locally — idempotent no-op
    }
    git_status(dir, &["branch", "-D", apply_ref])
        .await
        .map(|()| true)
        .map_err(|_| "failed to delete local apply branch".to_string())
}

/// Remote cleanup: the actual delete decision is a single atomic
/// `--force-with-lease` push, not a `ls-remote`-then-delete pair (which would have a TOCTOU
/// race for the delete decision — another actor could replace the ref between an `ls-remote`
/// read and an unconditional delete). An `ls-remote` existence check runs first, but only as
/// an idempotency short-circuit — and only a CONFIRMED-empty result (the call itself
/// succeeding with no matching ref) counts: `--force-with-lease` reports the SAME "stale info"
/// rejection whether the ref moved to a different SHA or was already deleted (verified
/// empirically), so without this check a second `reset` call on an already-cleaned-up branch
/// would be misreported as a CAS mismatch. If the `ls-remote` probe itself fails (network,
/// auth, ...), that failure is never treated as "confirmed absent" — it falls through to the
/// same atomic delete attempt, which is authoritative either way. The lease value MUST be one
/// glued `--force-with-lease=<ref>:<expect>` argv token — a space-separated form makes git
/// treat the flag as valueless and shifts the value into the remote-name positional instead.
async fn reset_remote(
    dir: &Path,
    apply_ref: &str,
    expected: &str,
    expected_push_url: Option<&str>,
) -> Result<bool, (nave_apply::ResetState, String)> {
    if let Some(expected_push_url) = expected_push_url {
        match git_output(dir, &["remote", "get-url", "--push", "origin"]).await {
            Ok(url) if url == expected_push_url => {}
            Ok(_) => {
                return Err((
                    nave_apply::ResetState::EvidenceMismatch,
                    "origin push url changed since provisioning".into(),
                ));
            }
            Err(e) => {
                return Err((
                    nave_apply::ResetState::EvidenceMismatch,
                    format!("could not verify origin push url: {e}"),
                ));
            }
        }
    }

    match git_output(
        dir,
        &["ls-remote", "origin", &format!("refs/heads/{apply_ref}")],
    )
    .await
    {
        Ok(ls) if ls.trim().is_empty() => return Ok(true), // confirmed absent — idempotent no-op
        Ok(_) | Err(_) => {} // present, or unknown — fall through to the authoritative delete
    }

    let lease = format!("--force-with-lease=refs/heads/{apply_ref}:{expected}");
    let out = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "push",
            &lease,
            "origin",
            &format!(":refs/heads/{apply_ref}"),
        ])
        .output()
        .await
        .map_err(|e| (nave_apply::ResetState::MissingBranch, e.to_string()))?;
    if out.status.success() {
        return Ok(true);
    }
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if stderr.contains("stale info") || stderr.contains("rejected") {
        return Err((
            nave_apply::ResetState::RemoteCasMismatch,
            "remote apply branch has moved since it was pushed — left intact".into(),
        ));
    }
    Err((
        nave_apply::ResetState::MissingBranch,
        format!("remote delete failed: {}", stderr.trim()),
    ))
}

async fn reset_one(
    dir: &Path,
    pen_branch: &str,
    apply_ref: &str,
    req: &nave_apply::ResetRepoRequest,
    repo_state: Option<&ApplyRepoState>,
) -> nave_apply::ResetRepoResult {
    if !dir.exists() {
        return nave_apply::ResetRepoResult {
            repo: req.repo.clone(),
            local_reset: false,
            remote_deleted: false,
            state: nave_apply::ResetState::MissingBranch,
            reason: Some("clone directory does not exist".into()),
        };
    }

    let (local_reset, mut reason) = match reset_local(dir, pen_branch, apply_ref).await {
        Ok(ok) => (ok, None),
        Err(reason) => (false, Some(reason)),
    };

    let mut remote_deleted = false;
    let mut state = nave_apply::ResetState::Ok;
    if reason.is_none() {
        if let Some(expected) = &req.expected_pushed_sha {
            let expected_push_url = repo_state.map(|s| s.expected_push_url.as_str());
            match reset_remote(dir, apply_ref, expected, expected_push_url).await {
                Ok(deleted) => remote_deleted = deleted,
                Err((s, msg)) => {
                    reason = Some(msg);
                    state = s;
                }
            }
        }
        // expected_pushed_sha == None: never pushed, nothing remote to clean up — idempotent.
    } else {
        state = nave_apply::ResetState::MissingBranch;
    }

    nave_apply::ResetRepoResult {
        repo: req.repo.clone(),
        local_reset,
        remote_deleted,
        state,
        reason,
    }
}

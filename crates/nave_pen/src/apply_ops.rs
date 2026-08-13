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

use crate::apply_state::{ApplyRepoState, read_apply_state, write_apply_state};
use crate::git_util::{git_ok, git_output, git_status};
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
                let result = provision_one(&dir, req, &request.apply_ref).await;
                if matches!(result.state, nave_apply::BranchState::Ok) {
                    let origin_url = git_output(&dir, &["remote", "get-url", "origin"])
                        .await
                        .unwrap_or_default();
                    let mut state = read_apply_state(pen_root, &pen.name, &request.apply_ref)?;
                    state.repos.insert(
                        req.repo.clone(),
                        ApplyRepoState {
                            base_ref: req.base_ref.clone(),
                            expected_base_sha: req.expected_base_sha.clone(),
                            expected_origin_url: origin_url,
                            local_commit_sha: None,
                        },
                    );
                    write_apply_state(pen_root, &pen.name, &request.apply_ref, &state)?;
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
    let porcelain = git_output(dir, &["status", "--porcelain"])
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
    let origin_url = git_output(dir, &["remote", "get-url", "origin"])
        .await
        .map_err(|e| (nave_apply::PushState::PushRejected, e.to_string()))?;
    if origin_url != repo_state.expected_origin_url {
        return Err((
            nave_apply::PushState::PushRejected,
            "origin remote url changed since provisioning".into(),
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
    if let Err(e) = git_status(
        dir,
        &[
            "push",
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
    let Ok(remote_sha) = git_output(dir, &["rev-parse", &format!("origin/{apply_ref}")]).await
    else {
        return Err((
            nave_apply::PushState::PushRejected,
            Some(branch_sha),
            "push succeeded but remote sha could not be verified".into(),
        ));
    };
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

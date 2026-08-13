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

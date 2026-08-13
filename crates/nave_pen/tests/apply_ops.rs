//! Integration tests for `nave_pen::apply_ops`. Kept as integration tests
//! (not `#[cfg(test)]` unit tests inside `src/apply_ops.rs`) deliberately:
//! `nave_test_support` is a dev-dependency of `nave_pen` that itself
//! depends on `nave_pen`, and passing a `nave_test_support`-constructed
//! `Pen` into a `nave_pen`-internal unit test hits Cargo's classic
//! self-referential-dev-dependency type-duplication error ("multiple
//! different versions of crate `nave_pen` in the dependency graph") — the
//! crate's own `#[cfg(test)]` build and the regular-dependency build
//! `nave_test_support` links against are distinct compilations. Integration
//! tests avoid this: they link `nave_pen` exactly once, the same instance
//! `nave_test_support` links.
use nave_apply::PROTOCOL_VERSION;
use nave_pen::apply_ops::{
    capabilities, commit_bound, provision_branch, push_branch, reset_branch,
};

/// Minimal local git-output helper for asserting on-disk state after a verb
/// call. `git_util`'s equivalent is crate-private and unreachable here.
async fn git_output(dir: &std::path::Path, args: &[&str]) -> String {
    let out = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

async fn git_status(dir: &std::path::Path, args: &[&str]) {
    let status = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .await
        .unwrap();
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

#[test]
fn capabilities_reports_protocol_and_verbs() {
    let caps = capabilities();
    assert_eq!(caps.protocol_version, PROTOCOL_VERSION);
    assert_eq!(
        caps.verbs,
        nave_apply::APPLY_VERBS
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );
    assert!(matches!(caps.adapter_state, nave_apply::AdapterState::Ok));
}

fn branch_req(fx: &nave_test_support::PenFixture, apply_ref: &str) -> nave_apply::BranchEnvelope {
    nave_apply::BranchEnvelope {
        protocol_version: PROTOCOL_VERSION,
        apply_ref: apply_ref.into(),
        repos: vec![nave_apply::BranchRepoRequest {
            repo: "acme/docs".into(),
            base_ref: "develop".into(),
            expected_base_sha: fx.base_sha.clone(),
        }],
    }
}

#[tokio::test]
async fn branch_provisions_off_verified_remote_base() {
    let fx = nave_test_support::init_pen_fixture("branch-fx", "acme", "docs", "develop").await;
    let res = provision_branch(
        fx.pen_root.path(),
        &fx.pen,
        &branch_req(&fx, "pulse/apply/p1"),
    )
    .await
    .unwrap();
    assert!(matches!(res.adapter_state, nave_apply::AdapterState::Ok));
    assert!(matches!(res.repos[0].state, nave_apply::BranchState::Ok));
    assert_eq!(res.repos[0].observed_base_sha, fx.base_sha);

    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "branch-fx", "acme", "docs");
    let branch = git_output(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await;
    assert_eq!(branch, "pulse/apply/p1");
}

#[tokio::test]
async fn branch_reports_stale_base_without_creating_branch() {
    let fx = nave_test_support::init_pen_fixture("branch-fx2", "acme", "docs", "develop").await;
    let mut req = branch_req(&fx, "pulse/apply/p1");
    req.repos[0].expected_base_sha = "0".repeat(40);
    let res = provision_branch(fx.pen_root.path(), &fx.pen, &req)
        .await
        .unwrap();
    assert!(matches!(
        res.repos[0].state,
        nave_apply::BranchState::StaleBase
    ));
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "branch-fx2", "acme", "docs");
    let branch = git_output(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await;
    assert_eq!(branch, fx.pen.branch);
}

#[tokio::test]
async fn branch_fails_closed_when_apply_ref_already_exists() {
    let fx = nave_test_support::init_pen_fixture("branch-fx3", "acme", "docs", "develop").await;
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "branch-fx3", "acme", "docs");
    git_status(&dir, &["checkout", "-B", "pulse/apply/p1"]).await;
    git_status(&dir, &["checkout", &fx.pen.branch]).await;
    let res = provision_branch(
        fx.pen_root.path(),
        &fx.pen,
        &branch_req(&fx, "pulse/apply/p1"),
    )
    .await
    .unwrap();
    assert!(matches!(
        res.repos[0].state,
        nave_apply::BranchState::Exists
    ));
}

#[tokio::test]
async fn branch_reports_unknown_repo_not_in_pen() {
    let fx = nave_test_support::init_pen_fixture("branch-fx4", "acme", "docs", "develop").await;
    let mut req = branch_req(&fx, "pulse/apply/p1");
    req.repos[0].repo = "other/repo".into();
    let res = provision_branch(fx.pen_root.path(), &fx.pen, &req)
        .await
        .unwrap();
    assert!(matches!(
        res.repos[0].state,
        nave_apply::BranchState::UnknownRepo
    ));
}

#[tokio::test]
async fn branch_rejects_invalid_ref_name_at_envelope_level() {
    let fx = nave_test_support::init_pen_fixture("branch-fx5", "acme", "docs", "develop").await;
    let mut req = branch_req(&fx, "../escape");
    req.apply_ref = "../escape".into();
    let res = provision_branch(fx.pen_root.path(), &fx.pen, &req)
        .await
        .unwrap();
    assert!(matches!(res.adapter_state, nave_apply::AdapterState::Error));
    assert!(res.repos.is_empty());
    assert!(res.reason.is_some());
}

async fn provisioned(name: &str, apply_ref: &str) -> nave_test_support::PenFixture {
    let fx = nave_test_support::init_pen_fixture(name, "acme", "docs", "develop").await;
    provision_branch(fx.pen_root.path(), &fx.pen, &branch_req(&fx, apply_ref))
        .await
        .unwrap();
    fx
}

fn commit_req(paths: &[&str]) -> nave_apply::CommitEnvelope {
    nave_apply::CommitEnvelope {
        protocol_version: PROTOCOL_VERSION,
        repos: vec![nave_apply::CommitRepoRequest {
            repo: "acme/docs".into(),
            paths: paths.iter().map(|s| (*s).to_string()).collect(),
        }],
    }
}

#[tokio::test]
async fn commit_stages_only_bound_paths() {
    let fx = provisioned("commit-fx", "pulse/apply/c1").await;
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "commit-fx", "acme", "docs");
    std::fs::write(dir.join("lockfile.json"), "{}").unwrap();
    let res = commit_bound(
        fx.pen_root.path(),
        &fx.pen,
        "pulse/apply/c1",
        "bump lockfile",
        &commit_req(&["lockfile.json"]),
    )
    .await
    .unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::CommitState::Ok));
    assert!(res.repos[0].local_commit_sha.is_some());
}

#[tokio::test]
async fn commit_fails_closed_on_dirty_path_outside_bounds() {
    let fx = provisioned("commit-fx2", "pulse/apply/c2").await;
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "commit-fx2", "acme", "docs");
    std::fs::write(dir.join("lockfile.json"), "{}").unwrap();
    std::fs::write(dir.join("unexpected.txt"), "surprise").unwrap();
    let res = commit_bound(
        fx.pen_root.path(),
        &fx.pen,
        "pulse/apply/c2",
        "m",
        &commit_req(&["lockfile.json"]),
    )
    .await
    .unwrap();
    assert!(matches!(
        res.repos[0].state,
        nave_apply::CommitState::DirtyOutsideBounds
    ));
    let status = git_output(&dir, &["status", "--porcelain"]).await;
    assert!(!status.is_empty(), "nothing should have been committed");
}

#[tokio::test]
async fn commit_fails_closed_when_origin_remote_changed() {
    let fx = provisioned("commit-fx3", "pulse/apply/c3").await;
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "commit-fx3", "acme", "docs");
    git_status(
        &dir,
        &["remote", "set-url", "origin", "file:///somewhere-else"],
    )
    .await;
    std::fs::write(dir.join("lockfile.json"), "{}").unwrap();
    let res = commit_bound(
        fx.pen_root.path(),
        &fx.pen,
        "pulse/apply/c3",
        "m",
        &commit_req(&["lockfile.json"]),
    )
    .await
    .unwrap();
    assert!(matches!(
        res.repos[0].state,
        nave_apply::CommitState::InvariantViolated
    ));
    assert!(
        res.repos[0]
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("origin")
    );
}

#[tokio::test]
async fn commit_fails_closed_when_no_apply_state_recorded() {
    let fx = nave_test_support::init_pen_fixture("commit-fx4", "acme", "docs", "develop").await;
    let res = commit_bound(
        fx.pen_root.path(),
        &fx.pen,
        "pulse/apply/never-provisioned",
        "m",
        &commit_req(&["x"]),
    )
    .await
    .unwrap();
    assert!(matches!(
        res.repos[0].state,
        nave_apply::CommitState::NoApplyState
    ));
}

#[tokio::test]
async fn commit_rejects_invalid_bound_path_at_envelope_level() {
    let fx = provisioned("commit-fx5", "pulse/apply/c5").await;
    let res = commit_bound(
        fx.pen_root.path(),
        &fx.pen,
        "pulse/apply/c5",
        "m",
        &commit_req(&["../escape"]),
    )
    .await
    .unwrap();
    assert!(matches!(res.adapter_state, nave_apply::AdapterState::Error));
    assert!(res.repos.is_empty());
}

async fn provisioned_and_committed(
    name: &str,
    apply_ref: &str,
) -> (nave_test_support::PenFixture, String) {
    let fx = provisioned(name, apply_ref).await;
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), name, "acme", "docs");
    std::fs::write(dir.join("lockfile.json"), "{}").unwrap();
    let commit_res = commit_bound(
        fx.pen_root.path(),
        &fx.pen,
        apply_ref,
        "m",
        &commit_req(&["lockfile.json"]),
    )
    .await
    .unwrap();
    (fx, commit_res.repos[0].local_commit_sha.clone().unwrap())
}

fn push_req() -> nave_apply::PushEnvelope {
    nave_apply::PushEnvelope {
        protocol_version: PROTOCOL_VERSION,
        repos: vec![nave_apply::PushRepoRequest {
            repo: "acme/docs".into(),
        }],
    }
}

#[tokio::test]
async fn push_reports_remote_sha_matching_local_commit() {
    let (fx, local_sha) = provisioned_and_committed("push-fx", "pulse/apply/pu1").await;
    let res = push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/pu1", &push_req())
        .await
        .unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::PushState::Ok));
    assert_eq!(res.repos[0].remote_sha.as_deref(), Some(local_sha.as_str()));
    assert_eq!(res.repos[0].remote_ref.as_deref(), Some("pulse/apply/pu1"));
}

#[tokio::test]
async fn push_is_idempotent_on_identical_history() {
    let (fx, _) = provisioned_and_committed("push-fx2", "pulse/apply/pu2").await;
    push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/pu2", &push_req())
        .await
        .unwrap();
    let second = push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/pu2", &push_req())
        .await
        .unwrap();
    assert!(matches!(second.repos[0].state, nave_apply::PushState::Ok));
}

#[tokio::test]
async fn push_fails_closed_without_a_prior_commit() {
    let fx = provisioned("push-fx3", "pulse/apply/pu3").await;
    let res = push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/pu3", &push_req())
        .await
        .unwrap();
    assert!(matches!(
        res.repos[0].state,
        nave_apply::PushState::NoApplyState
    ));
}

#[tokio::test]
async fn push_fails_closed_when_origin_remote_changed_since_commit() {
    let (fx, _) = provisioned_and_committed("push-fx4", "pulse/apply/pu4").await;
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "push-fx4", "acme", "docs");
    git_status(&dir, &["remote", "set-url", "origin", "file:///elsewhere"]).await;
    let res = push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/pu4", &push_req())
        .await
        .unwrap();
    assert!(matches!(
        res.repos[0].state,
        nave_apply::PushState::PushRejected
    ));
}

fn reset_req(expected_pushed_sha: Option<String>) -> nave_apply::ResetEnvelope {
    nave_apply::ResetEnvelope {
        protocol_version: PROTOCOL_VERSION,
        repos: vec![nave_apply::ResetRepoRequest {
            repo: "acme/docs".into(),
            expected_pushed_sha,
        }],
    }
}

#[tokio::test]
async fn reset_deletes_remote_ref_only_on_sha_match() {
    let (fx, local_sha) = provisioned_and_committed("reset-fx", "pulse/apply/r1").await;
    push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/r1", &push_req())
        .await
        .unwrap();

    let res = reset_branch(
        fx.pen_root.path(),
        &fx.pen,
        "pulse/apply/r1",
        &reset_req(Some(local_sha)),
    )
    .await
    .unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::ResetState::Ok));
    assert!(res.repos[0].local_reset);
    assert!(res.repos[0].remote_deleted);

    let remote_refs = git_output(
        fx.origin.path(),
        &["for-each-ref", "refs/heads/pulse/apply/r1"],
    )
    .await;
    assert!(remote_refs.is_empty());

    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "reset-fx", "acme", "docs");
    let branch = git_output(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await;
    assert_eq!(branch, fx.pen.branch);
}

#[tokio::test]
async fn reset_skips_remote_delete_on_cas_mismatch() {
    let (fx, _) = provisioned_and_committed("reset-fx2", "pulse/apply/r2").await;
    push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/r2", &push_req())
        .await
        .unwrap();

    let res = reset_branch(
        fx.pen_root.path(),
        &fx.pen,
        "pulse/apply/r2",
        &reset_req(Some("f".repeat(40))),
    )
    .await
    .unwrap();
    assert!(matches!(
        res.repos[0].state,
        nave_apply::ResetState::RemoteCasMismatch
    ));
    assert!(!res.repos[0].remote_deleted);
    let remote_refs = git_output(
        fx.origin.path(),
        &["for-each-ref", "refs/heads/pulse/apply/r2"],
    )
    .await;
    assert!(
        !remote_refs.is_empty(),
        "remote branch must survive a CAS mismatch"
    );
}

#[tokio::test]
async fn reset_is_idempotent_when_called_twice() {
    let (fx, local_sha) = provisioned_and_committed("reset-fx3", "pulse/apply/r3").await;
    push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/r3", &push_req())
        .await
        .unwrap();
    reset_branch(
        fx.pen_root.path(),
        &fx.pen,
        "pulse/apply/r3",
        &reset_req(Some(local_sha.clone())),
    )
    .await
    .unwrap();
    let second = reset_branch(
        fx.pen_root.path(),
        &fx.pen,
        "pulse/apply/r3",
        &reset_req(Some(local_sha)),
    )
    .await
    .unwrap();
    assert!(matches!(second.repos[0].state, nave_apply::ResetState::Ok));
    assert!(second.repos[0].local_reset);
}

#[tokio::test]
async fn reset_handles_never_pushed_repo_without_touching_remote() {
    let fx = provisioned("reset-fx4", "pulse/apply/r4").await;
    let res = reset_branch(
        fx.pen_root.path(),
        &fx.pen,
        "pulse/apply/r4",
        &reset_req(None),
    )
    .await
    .unwrap();
    assert!(matches!(res.repos[0].state, nave_apply::ResetState::Ok));
    assert!(res.repos[0].local_reset);
    assert!(!res.repos[0].remote_deleted);
}

#[tokio::test]
async fn branch_request_with_two_repos_reports_independent_outcomes() {
    let mut fx =
        nave_test_support::init_pen_fixture("branch-multi", "acme", "docs", "develop").await;
    let (_origin_b, base_sha_b) =
        nave_test_support::add_repo_to_fixture(&mut fx, "acme", "web", "develop").await;

    let req = nave_apply::BranchEnvelope {
        protocol_version: PROTOCOL_VERSION,
        apply_ref: "pulse/apply/multi".into(),
        repos: vec![
            nave_apply::BranchRepoRequest {
                repo: "acme/docs".into(),
                base_ref: "develop".into(),
                expected_base_sha: fx.base_sha.clone(),
            },
            // Deliberately wrong expected SHA for the second repo — proves one repo's failure
            // doesn't block or corrupt the other's success within the same request.
            nave_apply::BranchRepoRequest {
                repo: "acme/web".into(),
                base_ref: "develop".into(),
                expected_base_sha: "0".repeat(40),
            },
        ],
    };
    let res = provision_branch(fx.pen_root.path(), &fx.pen, &req)
        .await
        .unwrap();
    assert!(matches!(res.adapter_state, nave_apply::AdapterState::Ok));
    assert_eq!(res.repos.len(), 2);
    let docs = res.repos.iter().find(|r| r.repo == "acme/docs").unwrap();
    let web = res.repos.iter().find(|r| r.repo == "acme/web").unwrap();
    assert!(matches!(docs.state, nave_apply::BranchState::Ok));
    assert!(matches!(web.state, nave_apply::BranchState::StaleBase));

    let dir_docs = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "branch-multi", "acme", "docs");
    assert_eq!(
        git_output(&dir_docs, &["rev-parse", "--abbrev-ref", "HEAD"]).await,
        "pulse/apply/multi"
    );
    let dir_web = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "branch-multi", "acme", "web");
    assert_eq!(
        git_output(&dir_web, &["rev-parse", "--abbrev-ref", "HEAD"]).await,
        fx.pen.branch
    );
    let _ = base_sha_b;
}

#[tokio::test]
async fn branch_request_with_duplicate_repo_is_rejected_before_any_mutation() {
    let fx = nave_test_support::init_pen_fixture("branch-dup", "acme", "docs", "develop").await;
    let mut req = branch_req(&fx, "pulse/apply/dup");
    req.repos.push(req.repos[0].clone());
    let res = provision_branch(fx.pen_root.path(), &fx.pen, &req)
        .await
        .unwrap();
    assert!(matches!(res.adapter_state, nave_apply::AdapterState::Error));
    assert!(res.repos.is_empty());

    // No mutation happened: still on the pen's own branch, apply_ref never created.
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "branch-dup", "acme", "docs");
    assert_eq!(
        git_output(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await,
        fx.pen.branch
    );
    assert!(
        !nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "branch-dup", "acme", "docs")
            .join(".git")
            .join("refs/heads/pulse")
            .exists()
    );
}

#[tokio::test]
async fn ref_name_with_revision_shorthand_is_rejected() {
    let fx =
        nave_test_support::init_pen_fixture("branch-shorthand", "acme", "docs", "develop").await;
    let mut req = branch_req(&fx, "@{-1}");
    req.apply_ref = "@{-1}".into();
    let res = provision_branch(fx.pen_root.path(), &fx.pen, &req)
        .await
        .unwrap();
    assert!(matches!(res.adapter_state, nave_apply::AdapterState::Error));
    assert!(res.repos.is_empty());
}

#[tokio::test]
async fn push_fails_closed_when_only_the_push_url_changed() {
    // A fetch-URL-only check would miss this: `git push` uses `remote.origin.pushurl` when
    // set, independent of the fetch URL — this is exactly the bypass the closing review found.
    let (fx, _) = provisioned_and_committed("push-pushurl", "pulse/apply/pu5").await;
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "push-pushurl", "acme", "docs");
    let fetch_before = git_output(&dir, &["remote", "get-url", "origin"]).await;
    git_status(
        &dir,
        &[
            "config",
            "remote.origin.pushurl",
            "file:///elsewhere-push-only",
        ],
    )
    .await;
    let fetch_after = git_output(&dir, &["remote", "get-url", "origin"]).await;
    assert_eq!(
        fetch_before, fetch_after,
        "fetch url must be unchanged by this test's setup"
    );

    let res = push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/pu5", &push_req())
        .await
        .unwrap();
    assert!(matches!(
        res.repos[0].state,
        nave_apply::PushState::PushRejected
    ));
}

#[tokio::test]
async fn reset_fails_closed_when_the_push_url_changed_since_provisioning() {
    let (fx, local_sha) = provisioned_and_committed("reset-pushurl", "pulse/apply/r5").await;
    push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/r5", &push_req())
        .await
        .unwrap();
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "reset-pushurl", "acme", "docs");
    git_status(
        &dir,
        &[
            "config",
            "remote.origin.pushurl",
            "file:///elsewhere-push-only",
        ],
    )
    .await;

    let res = reset_branch(
        fx.pen_root.path(),
        &fx.pen,
        "pulse/apply/r5",
        &reset_req(Some(local_sha)),
    )
    .await
    .unwrap();
    assert!(matches!(
        res.repos[0].state,
        nave_apply::ResetState::EvidenceMismatch
    ));
    assert!(!res.repos[0].remote_deleted);
    let remote_refs = git_output(
        fx.origin.path(),
        &["for-each-ref", "refs/heads/pulse/apply/r5"],
    )
    .await;
    assert!(
        !remote_refs.is_empty(),
        "remote branch must survive an evidence mismatch"
    );
}

#[tokio::test]
async fn commit_fails_closed_when_checked_out_branch_changed() {
    let fx = provisioned("commit-branch-changed", "pulse/apply/c6").await;
    let dir =
        nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "commit-branch-changed", "acme", "docs");
    // Simulate an ecosystem command switching branches mid-exec.
    git_status(&dir, &["checkout", &fx.pen.branch]).await;
    let res = commit_bound(
        fx.pen_root.path(),
        &fx.pen,
        "pulse/apply/c6",
        "m",
        &commit_req(&["lockfile.json"]),
    )
    .await
    .unwrap();
    assert!(matches!(
        res.repos[0].state,
        nave_apply::CommitState::InvariantViolated
    ));
    assert!(
        res.repos[0]
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("branch")
    );
}

#[tokio::test]
async fn commit_fails_closed_when_head_moved_since_provisioning() {
    let fx = provisioned("commit-head-moved", "pulse/apply/c7").await;
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "commit-head-moved", "acme", "docs");
    // Simulate an ecosystem command self-committing during exec.
    std::fs::write(dir.join("surprise.txt"), "x").unwrap();
    git_status(&dir, &["add", "surprise.txt"]).await;
    git_status(
        &dir,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-m",
            "unexpected",
        ],
    )
    .await;
    let res = commit_bound(
        fx.pen_root.path(),
        &fx.pen,
        "pulse/apply/c7",
        "m",
        &commit_req(&["lockfile.json"]),
    )
    .await
    .unwrap();
    assert!(matches!(
        res.repos[0].state,
        nave_apply::CommitState::InvariantViolated
    ));
    assert!(
        res.repos[0]
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("HEAD")
    );
}

#[tokio::test]
async fn reset_does_not_silently_succeed_when_ls_remote_cannot_reach_origin() {
    let (fx, local_sha) = provisioned_and_committed("reset-unreachable", "pulse/apply/r6").await;
    push_branch(fx.pen_root.path(), &fx.pen, "pulse/apply/r6", &push_req())
        .await
        .unwrap();
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "reset-unreachable", "acme", "docs");
    // Point origin at a path that cannot be reached at all (not just "empty") — a failing,
    // not an empty-succeeding, `ls-remote`. The old `unwrap_or_default()` bug treated this
    // identically to a confirmed-absent branch and reported false success.
    git_status(
        &dir,
        &[
            "remote",
            "set-url",
            "origin",
            "file:///nonexistent/path/that/does/not/exist.git",
        ],
    )
    .await;

    let res = reset_branch(
        fx.pen_root.path(),
        &fx.pen,
        "pulse/apply/r6",
        &reset_req(Some(local_sha)),
    )
    .await
    .unwrap();
    assert!(
        !matches!(res.repos[0].state, nave_apply::ResetState::Ok),
        "an unreachable origin must never report ok"
    );
    assert!(!res.repos[0].remote_deleted);
}

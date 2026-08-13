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
use nave_pen::apply_ops::{capabilities, commit_bound, provision_branch, push_branch};

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

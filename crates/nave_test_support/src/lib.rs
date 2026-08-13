//! Real-git fixtures for pen apply-verb tests: a bare "origin" repo plus a
//! pen clone laid out and checked out exactly like `create_pen` produces.

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
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .await
        .unwrap();
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

/// Bare "origin" repo, seeded with one commit on `default_branch`, plus a pen clone under
/// `pen_root` checked out onto `pen_branch` — the shared machinery behind `init_pen_fixture`
/// (first repo) and `add_repo_to_fixture` (subsequent repos in a multi-repo pen).
async fn seed_repo_in_pen(
    pen_root: &std::path::Path,
    pen_name: &str,
    pen_branch: &str,
    owner: &str,
    repo: &str,
    default_branch: &str,
) -> (TempDir, String) {
    let origin = TempDir::new().unwrap();
    git(origin.path(), &["init", "--bare", "-b", default_branch]).await;

    let seed = TempDir::new().unwrap();
    git(
        seed.path(),
        &["clone", origin.path().to_str().unwrap(), "."],
    )
    .await;
    std::fs::write(seed.path().join("README.md"), "seed\n").unwrap();
    git(seed.path(), &["add", "README.md"]).await;
    git(
        seed.path(),
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-m",
            "seed",
        ],
    )
    .await;
    git(seed.path(), &["push", "origin", default_branch]).await;
    let sha_out = Command::new("git")
        .arg("-C")
        .arg(seed.path())
        .args(["rev-parse", "HEAD"])
        .output()
        .await
        .unwrap();
    let base_sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();

    let clone_dir: PathBuf = nave_pen::pen_repo_clone_dir(pen_root, pen_name, owner, repo);
    git(
        pen_root,
        &[
            "clone",
            origin.path().to_str().unwrap(),
            clone_dir.to_str().unwrap(),
        ],
    )
    .await;
    // Mirror create_pen's clone_and_branch exactly: checkout -b <pen_branch> <default_branch>,
    // falling back to a plain checkout if the branch already exists.
    let checkout = Command::new("git")
        .arg("-C")
        .arg(&clone_dir)
        .args(["checkout", "-b", pen_branch, default_branch])
        .status()
        .await
        .unwrap();
    if !checkout.success() {
        git(&clone_dir, &["checkout", pen_branch]).await;
    }
    git(&clone_dir, &["config", "user.email", "t@t"]).await;
    git(&clone_dir, &["config", "user.name", "t"]).await;

    (origin, base_sha)
}

pub async fn init_pen_fixture(
    pen_name: &str,
    owner: &str,
    repo: &str,
    default_branch: &str,
) -> PenFixture {
    let pen_root = TempDir::new().unwrap();
    let pen_branch = format!("nave/{pen_name}");
    let (origin, base_sha) = seed_repo_in_pen(
        pen_root.path(),
        pen_name,
        &pen_branch,
        owner,
        repo,
        default_branch,
    )
    .await;

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

    PenFixture {
        origin,
        pen_root,
        pen,
        base_sha,
    }
}

/// Adds a second (or Nth) repo to an existing single-repo (or multi-repo) fixture's pen —
/// its own bare origin, its own clone under the same `pen_root`, checked out onto the same
/// pen branch — and re-persists `pen.toml`. Returns the new repo's origin `TempDir` (the
/// caller must hold onto it; it deletes the bare repo on drop) and its seeded base SHA.
/// `PenFixture` itself stays single-`origin`/single-`base_sha` for the common case; multi-repo
/// tests track the extra origins/SHAs themselves.
pub async fn add_repo_to_fixture(
    fx: &mut PenFixture,
    owner: &str,
    repo: &str,
    default_branch: &str,
) -> (TempDir, String) {
    let (origin, base_sha) = seed_repo_in_pen(
        fx.pen_root.path(),
        &fx.pen.name,
        &fx.pen.branch,
        owner,
        repo,
        default_branch,
    )
    .await;
    fx.pen.repos.push(PenRepo {
        owner: owner.to_string(),
        name: repo.to_string(),
        default_branch: default_branch.to_string(),
        clone_url: format!("file://{}", origin.path().display()),
        synced_at: OffsetDateTime::now_utc(),
    });
    nave_pen::storage::write_pen(fx.pen_root.path(), &fx.pen).unwrap();
    (origin, base_sha)
}

#[tokio::test]
async fn fixture_clone_is_checked_out_on_pen_branch() {
    let fx = init_pen_fixture("apply-fixture", "acme", "docs", "develop").await;
    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "apply-fixture", "acme", "docs");
    let out = tokio::process::Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), fx.pen.branch);
    let loaded = nave_pen::load_pen(fx.pen_root.path(), "apply-fixture").unwrap();
    assert_eq!(loaded.name, "apply-fixture");
}

#[tokio::test]
async fn add_repo_to_fixture_appends_a_second_working_repo() {
    let mut fx = init_pen_fixture("multi-fixture", "acme", "docs", "develop").await;
    let (_origin_b, base_sha_b) = add_repo_to_fixture(&mut fx, "acme", "web", "develop").await;
    assert_eq!(fx.pen.repos.len(), 2);
    let dir_b = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "multi-fixture", "acme", "web");
    let out = tokio::process::Command::new("git")
        .arg("-C")
        .arg(&dir_b)
        .args(["rev-parse", "HEAD"])
        .output()
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), base_sha_b);
    let loaded = nave_pen::load_pen(fx.pen_root.path(), "multi-fixture").unwrap();
    assert_eq!(loaded.repos.len(), 2);
}

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

pub async fn init_pen_fixture(
    pen_name: &str,
    owner: &str,
    repo: &str,
    default_branch: &str,
) -> PenFixture {
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

    let pen_root = TempDir::new().unwrap();
    let clone_dir: PathBuf = nave_pen::pen_repo_clone_dir(pen_root.path(), pen_name, owner, repo);
    git(
        pen_root.path(),
        &[
            "clone",
            origin.path().to_str().unwrap(),
            clone_dir.to_str().unwrap(),
        ],
    )
    .await;
    let pen_branch = format!("nave/{pen_name}");
    // Mirror create_pen's clone_and_branch exactly: checkout -b <pen_branch> <default_branch>,
    // falling back to a plain checkout if the branch already exists.
    let checkout = Command::new("git")
        .arg("-C")
        .arg(&clone_dir)
        .args(["checkout", "-b", &pen_branch, default_branch])
        .status()
        .await
        .unwrap();
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

    PenFixture {
        origin,
        pen_root,
        pen,
        base_sha,
    }
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

//! Shared git-plumbing helpers for apply-verb operations. Distinct from
//! `ops.rs`'s fire-and-forget helpers: every function here returns captured
//! stdout, because the apply verbs report exact SHAs/refs back to the
//! caller as structured JSON — git's own text output is the only place
//! that data exists.
//!
//! `#[allow(dead_code)]`: no non-test consumer exists until Task 6's
//! `provision_branch` (`apply_ops.rs`) is implemented — removed there.
#![allow(dead_code)]

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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn git_output_captures_trimmed_stdout() {
        let fx = nave_test_support::init_pen_fixture("git-util-fx", "acme", "docs", "main").await;
        let dir = crate::pen_repo_clone_dir(fx.pen_root.path(), "git-util-fx", "acme", "docs");
        let sha = git_output(&dir, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(sha, fx.base_sha);
    }

    #[tokio::test]
    async fn git_output_bails_on_nonzero_exit() {
        let fx = nave_test_support::init_pen_fixture("git-util-fx2", "acme", "docs", "main").await;
        let dir = crate::pen_repo_clone_dir(fx.pen_root.path(), "git-util-fx2", "acme", "docs");
        assert!(git_output(&dir, &["rev-parse", "refs/heads/does-not-exist"]).await.is_err());
    }

    #[tokio::test]
    async fn git_ok_is_false_not_error_on_missing_ref() {
        let fx = nave_test_support::init_pen_fixture("git-util-fx3", "acme", "docs", "main").await;
        let dir = crate::pen_repo_clone_dir(fx.pen_root.path(), "git-util-fx3", "acme", "docs");
        let ok = git_ok(&dir, &["rev-parse", "--verify", "--quiet", "refs/heads/nope"]).await.unwrap();
        assert!(!ok);
    }
}

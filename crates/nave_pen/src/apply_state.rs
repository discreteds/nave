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
    pen_dir(pen_root, pen_name)
        .join("apply")
        .join(apply_ref)
        .join("state.toml")
}

pub(crate) fn read_apply_state(
    pen_root: &Path,
    pen_name: &str,
    apply_ref: &str,
) -> Result<ApplyState> {
    let path = apply_state_path(pen_root, pen_name, apply_ref);
    if !path.exists() {
        return Ok(ApplyState::default());
    }
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

pub(crate) fn write_apply_state(
    pen_root: &Path,
    pen_name: &str,
    apply_ref: &str,
    state: &ApplyState,
) -> Result<()> {
    let path = apply_state_path(pen_root, pen_name, apply_ref);
    let parent = path.parent().expect("apply state path always has a parent");
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".state.toml.tmp.{}", std::process::id()));
    std::fs::write(&tmp, toml::to_string_pretty(state)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("renaming into {}", path.display()))
}

pub(crate) fn clear_apply_state(pen_root: &Path, pen_name: &str, apply_ref: &str) -> Result<()> {
    let path = apply_state_path(pen_root, pen_name, apply_ref);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ApplyRepoState {
        ApplyRepoState {
            base_ref: "develop".into(),
            expected_base_sha: "a".repeat(40),
            expected_origin_url: "file:///origin".into(),
            local_commit_sha: None,
        }
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
        write_apply_state(
            root.path(),
            "pen1",
            "pulse/apply/p1",
            &ApplyState::default(),
        )
        .unwrap();
        clear_apply_state(root.path(), "pen1", "pulse/apply/p1").unwrap();
        assert!(
            read_apply_state(root.path(), "pen1", "pulse/apply/p1")
                .unwrap()
                .repos
                .is_empty()
        );
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
        assert_eq!(
            read_apply_state(root.path(), "pen1", "pulse/a__b/c")
                .unwrap()
                .repos
                .len(),
            1
        );
        assert_eq!(
            read_apply_state(root.path(), "pen1", "pulse/a/b__c")
                .unwrap()
                .repos
                .len(),
            1
        );
        assert!(
            read_apply_state(root.path(), "pen1", "pulse/a__b/c")
                .unwrap()
                .repos
                .contains_key("x/y")
        );
        assert!(
            read_apply_state(root.path(), "pen1", "pulse/a/b__c")
                .unwrap()
                .repos
                .contains_key("p/q")
        );
    }
}

//! Per-pen, per-apply-branch sidecar: the base SHA and origin fetch/push URLs
//! `provision_branch` verified, persisted because each verb runs as a
//! separate process. `apply_ref` is percent-encoded (`/`→`%2F`, `%`→`%25`, in that
//! escape order so the escape character itself is always escaped) into a single flat
//! filename — not nested directory components, which (even after excluding the earlier
//! lossy `/`→`__` collapse) still let one ref's reserved `state.toml` leaf collide with
//! another, longer ref that legitimately has a path component named `state.toml`. Standard
//! percent-encoding of both the separator and the escape character is injective: distinct
//! refs always encode to distinct filenames, with no reserved substring collision possible.

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
    /// `git remote get-url origin` at provisioning time — the fetch URL, checked by `commit`
    /// as a defensive "has anything about origin changed" signal.
    pub expected_origin_url: String,
    /// `git remote get-url --push origin` at provisioning time — the URL `git push` actually
    /// uses (falls back to the fetch URL when no separate `remote.origin.pushurl` is
    /// configured). Checked by `push`/`reset`, since those are the operations a changed
    /// pushurl would actually redirect.
    pub expected_push_url: String,
    #[serde(default)]
    pub local_commit_sha: Option<String>,
}

fn encode_apply_ref(apply_ref: &str) -> String {
    apply_ref.replace('%', "%25").replace('/', "%2F")
}

fn apply_state_path(pen_root: &Path, pen_name: &str, apply_ref: &str) -> PathBuf {
    pen_dir(pen_root, pen_name)
        .join("apply")
        .join(format!("{}.toml", encode_apply_ref(apply_ref)))
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
            expected_push_url: "file:///origin".into(),
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

    /// A prior nested-directory encoding could let a short ref's on-disk sidecar file collide
    /// with a directory a longer, reserved-name-containing ref needed (e.g. `pulse/a` writing
    /// `apply/pulse/a/state.toml` while `pulse/a/state.toml/b` needed `apply/pulse/a/state.toml`
    /// to be a directory, not that same file). The flat percent-encoded scheme has no
    /// directory nesting at all, so this can't happen — both refs get their own single file.
    #[test]
    fn ref_containing_reserved_leaf_name_does_not_collide_with_a_shorter_ref() {
        let root = tempfile::TempDir::new().unwrap();
        let mut short = ApplyState::default();
        short.repos.insert("x/y".into(), sample());
        write_apply_state(root.path(), "pen1", "pulse/a", &short).unwrap();

        let mut long = ApplyState::default();
        long.repos.insert("p/q".into(), sample());
        write_apply_state(root.path(), "pen1", "pulse/a/state.toml/b", &long).unwrap();

        assert!(
            read_apply_state(root.path(), "pen1", "pulse/a")
                .unwrap()
                .repos
                .contains_key("x/y")
        );
        assert!(
            read_apply_state(root.path(), "pen1", "pulse/a/state.toml/b")
                .unwrap()
                .repos
                .contains_key("p/q")
        );
    }

    #[test]
    fn encoding_is_injective_for_percent_and_slash() {
        // A literal "%2F" in a ref must not be confusable with an escaped "/".
        assert_ne!(
            encode_apply_ref("pulse/apply"),
            encode_apply_ref("pulse%2Fapply")
        );
    }
}

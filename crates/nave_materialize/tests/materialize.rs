//! Materializer tests driven by an in-memory fake [`MaterializeSource`].
//!
//! No network: a `FakeSource` holds maps of repos, trees, and blobs keyed by
//! identity, and can simulate GitHub failures (404/403) by returning an
//! `anyhow::Error` from the relevant method. These tests pin the per-repo /
//! per-selector / per-file state machine and the deterministic ordering of
//! the emitted [`MaterializeResult`].

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use base64::Engine;
use nave_github::{BlobResponse, Repo, RepoOwner, TreeEntry, TreeResponse};
use nave_materialize::{
    ArtifactState, MAX_FILE_BYTES, MaterializeRequest, MaterializeSource, RepoRequest, Selector,
    materialize,
};

// --- fake source ------------------------------------------------------------

#[derive(Default)]
struct FakeSource {
    repos: HashMap<String, Result<Repo, String>>,
    trees: HashMap<String, Result<TreeResponse, String>>,
    blobs: HashMap<String, Result<BlobResponse, String>>,
}

impl FakeSource {
    fn with_repo(mut self, owner: &str, name: &str, default_branch: &str) -> Self {
        self.repos.insert(
            format!("{owner}/{name}"),
            Ok(make_repo(owner, name, default_branch)),
        );
        self
    }

    fn with_repo_error(mut self, owner: &str, name: &str, msg: &str) -> Self {
        self.repos
            .insert(format!("{owner}/{name}"), Err(msg.to_string()));
        self
    }

    fn with_tree(mut self, owner: &str, name: &str, tree: TreeResponse) -> Self {
        self.trees.insert(format!("{owner}/{name}"), Ok(tree));
        self
    }

    fn with_tree_error(mut self, owner: &str, name: &str, msg: &str) -> Self {
        self.trees
            .insert(format!("{owner}/{name}"), Err(msg.to_string()));
        self
    }

    fn with_blob(mut self, blob: BlobResponse) -> Self {
        self.blobs.insert(blob.sha.clone(), Ok(blob));
        self
    }

    fn with_blob_error(mut self, sha: &str, msg: &str) -> Self {
        self.blobs.insert(sha.to_string(), Err(msg.to_string()));
        self
    }
}

impl MaterializeSource for FakeSource {
    async fn repository(&self, owner: &str, repo: &str) -> Result<Repo> {
        match self.repos.get(&format!("{owner}/{repo}")) {
            Some(Ok(r)) => Ok(r.clone()),
            Some(Err(e)) => Err(anyhow!(e.clone())),
            None => Err(anyhow!("no repo fixture for {owner}/{repo}")),
        }
    }

    async fn tree(&self, owner: &str, repo: &str, _ref_name: &str) -> Result<TreeResponse> {
        match self.trees.get(&format!("{owner}/{repo}")) {
            Some(Ok(t)) => Ok(t.clone()),
            Some(Err(e)) => Err(anyhow!(e.clone())),
            None => Err(anyhow!("no tree fixture for {owner}/{repo}")),
        }
    }

    async fn blob(&self, _owner: &str, _repo: &str, sha: &str) -> Result<BlobResponse> {
        match self.blobs.get(sha) {
            Some(Ok(b)) => Ok(b.clone()),
            Some(Err(e)) => Err(anyhow!(e.clone())),
            None => Err(anyhow!("no blob fixture for {sha}")),
        }
    }
}

// --- fixtures ---------------------------------------------------------------

fn make_repo(owner: &str, name: &str, default_branch: &str) -> Repo {
    Repo {
        name: name.to_string(),
        full_name: format!("{owner}/{name}"),
        default_branch: default_branch.to_string(),
        clone_url: format!("https://github.com/{owner}/{name}.git"),
        fork: false,
        archived: false,
        pushed_at: None,
        owner: RepoOwner {
            login: owner.to_string(),
        },
    }
}

fn blob_entry(path: &str, sha: &str) -> TreeEntry {
    TreeEntry {
        path: path.to_string(),
        entry_type: "blob".to_string(),
        sha: sha.to_string(),
    }
}

fn dir_entry(path: &str, sha: &str) -> TreeEntry {
    TreeEntry {
        path: path.to_string(),
        entry_type: "tree".to_string(),
        sha: sha.to_string(),
    }
}

fn tree(sha: &str, truncated: bool, entries: Vec<TreeEntry>) -> TreeResponse {
    TreeResponse {
        sha: sha.to_string(),
        tree: entries,
        truncated,
    }
}

/// A well-formed Base64 blob of the given text with an honest declared size.
fn text_blob(sha: &str, text: &str) -> BlobResponse {
    BlobResponse {
        sha: sha.to_string(),
        size: text.len() as u64,
        encoding: "base64".to_string(),
        content: base64::engine::general_purpose::STANDARD.encode(text.as_bytes()),
    }
}

/// A Base64 blob of raw bytes with an explicitly declared size.
fn bytes_blob(sha: &str, bytes: &[u8], declared_size: u64) -> BlobResponse {
    BlobResponse {
        sha: sha.to_string(),
        size: declared_size,
        encoding: "base64".to_string(),
        content: base64::engine::general_purpose::STANDARD.encode(bytes),
    }
}

fn request(repo: &str, selectors: Vec<Selector>) -> MaterializeRequest {
    MaterializeRequest {
        contract_version: 1,
        repos: vec![RepoRequest {
            repo: repo.to_string(),
            selectors,
        }],
    }
}

fn selector(id: &str, pattern: &str, max_bytes: Option<u64>) -> Selector {
    Selector {
        id: id.to_string(),
        pattern: pattern.to_string(),
        max_bytes,
    }
}

// --- tests ------------------------------------------------------------------

#[tokio::test]
async fn exact_match_produces_found_with_content() {
    let src = FakeSource::default()
        .with_repo("acme", "widget", "main")
        .with_tree(
            "acme",
            "widget",
            tree("t1", false, vec![blob_entry("README.md", "b1")]),
        )
        .with_blob(text_blob("b1", "hello world"));

    let result = materialize(
        &src,
        request("acme/widget", vec![selector("readme", "README.md", None)]),
    )
    .await;

    assert_eq!(result.repos.len(), 1);
    let repo = &result.repos[0];
    assert_eq!(repo.repo, "acme/widget");
    assert_eq!(repo.ref_name, "main");
    assert_eq!(repo.tree_sha, "t1");
    assert!(repo.tree_complete);
    assert_eq!(repo.artifacts.len(), 1);

    let art = &repo.artifacts[0];
    assert_eq!(art.selector_id, "readme");
    assert_eq!(art.state, ArtifactState::Found);
    assert_eq!(art.path.as_deref(), Some("README.md"));
    assert_eq!(art.blob_sha.as_deref(), Some("b1"));
    assert_eq!(art.size_bytes, Some(11));
    assert_eq!(art.encoding.as_deref(), Some("utf-8"));
    assert_eq!(art.content.as_deref(), Some("hello world"));
}

#[tokio::test]
async fn glob_fan_out_yields_sorted_found_sharing_selector_id() {
    let src = FakeSource::default()
        .with_repo("acme", "widget", "main")
        .with_tree(
            "acme",
            "widget",
            tree(
                "t1",
                false,
                vec![
                    blob_entry("src/z.rs", "bz"),
                    blob_entry("src/a.rs", "ba"),
                    blob_entry("src/m.rs", "bm"),
                    dir_entry("src/nested", "d1"),
                    blob_entry("README.md", "br"),
                ],
            ),
        )
        .with_blob(text_blob("bz", "z"))
        .with_blob(text_blob("ba", "a"))
        .with_blob(text_blob("bm", "m"));

    let result = materialize(
        &src,
        request("acme/widget", vec![selector("rs", "src/**/*.rs", None)]),
    )
    .await;

    let arts = &result.repos[0].artifacts;
    assert_eq!(arts.len(), 3, "three .rs blobs, directory excluded");
    for art in arts {
        assert_eq!(art.selector_id, "rs");
        assert_eq!(art.state, ArtifactState::Found);
    }
    let paths: Vec<_> = arts.iter().map(|a| a.path.as_deref().unwrap()).collect();
    assert_eq!(paths, vec!["src/a.rs", "src/m.rs", "src/z.rs"]);
}

#[tokio::test]
async fn authoritative_absence_on_complete_tree() {
    let src = FakeSource::default()
        .with_repo("acme", "widget", "main")
        .with_tree(
            "acme",
            "widget",
            tree("t1", false, vec![blob_entry("README.md", "b1")]),
        )
        .with_blob(text_blob("b1", "x"));

    let result = materialize(
        &src,
        request(
            "acme/widget",
            vec![selector("missing", "does/not/exist.txt", None)],
        ),
    )
    .await;

    let arts = &result.repos[0].artifacts;
    assert_eq!(arts.len(), 1);
    assert_eq!(arts[0].state, ArtifactState::Absent);
    assert_eq!(arts[0].path, None);
    assert_eq!(arts[0].content, None);
}

#[tokio::test]
async fn truncated_tree_no_match_is_unresolved_not_absent() {
    let src = FakeSource::default()
        .with_repo("acme", "widget", "main")
        .with_tree(
            "acme",
            "widget",
            tree("t1", true, vec![blob_entry("README.md", "b1")]),
        )
        .with_blob(text_blob("b1", "x"));

    let result = materialize(
        &src,
        request(
            "acme/widget",
            vec![selector("missing", "config/app.toml", None)],
        ),
    )
    .await;

    let repo = &result.repos[0];
    assert!(!repo.tree_complete);
    assert_eq!(repo.artifacts.len(), 1);
    assert_eq!(repo.artifacts[0].state, ArtifactState::Unresolved);
    assert_eq!(repo.artifacts[0].path, None);
}

#[tokio::test]
async fn missing_repository_is_error_not_absent() {
    let src =
        FakeSource::default().with_repo_error("acme", "ghost", "GitHub returned 404 Not Found");

    let result = materialize(
        &src,
        request("acme/ghost", vec![selector("readme", "README.md", None)]),
    )
    .await;

    let repo = &result.repos[0];
    assert_eq!(repo.artifacts.len(), 1);
    assert_eq!(repo.artifacts[0].state, ArtifactState::Error);
    assert_ne!(repo.artifacts[0].state, ArtifactState::Absent);
    assert!(repo.artifacts[0].detail.is_some());
}

#[tokio::test]
async fn rate_limited_tree_is_error() {
    let src = FakeSource::default()
        .with_repo("acme", "widget", "main")
        .with_tree_error(
            "acme",
            "widget",
            "GitHub returned 403 (x-ratelimit-remaining=0)",
        );

    let result = materialize(
        &src,
        request("acme/widget", vec![selector("readme", "README.md", None)]),
    )
    .await;

    let repo = &result.repos[0];
    assert_eq!(repo.artifacts.len(), 1);
    assert_eq!(repo.artifacts[0].state, ArtifactState::Error);
    assert!(repo.artifacts[0].detail.as_deref().unwrap().contains("403"));
}

#[tokio::test]
async fn blob_fetch_error_is_error_state() {
    let src = FakeSource::default()
        .with_repo("acme", "widget", "main")
        .with_tree(
            "acme",
            "widget",
            tree("t1", false, vec![blob_entry("README.md", "b1")]),
        )
        .with_blob_error("b1", "GitHub returned 403 (x-ratelimit-remaining=0)");

    let result = materialize(
        &src,
        request("acme/widget", vec![selector("readme", "README.md", None)]),
    )
    .await;

    let art = &result.repos[0].artifacts[0];
    assert_eq!(art.state, ArtifactState::Error);
    assert_eq!(art.path.as_deref(), Some("README.md"));
    assert_eq!(art.content, None);
}

#[tokio::test]
async fn oversized_declared_blob_is_too_large_without_decoding() {
    // Declared size exceeds the limit, and the content is deliberately invalid
    // Base64: if the implementation decoded before checking declared size it
    // would surface an Error, so TooLarge here proves the decode was skipped.
    let src = FakeSource::default()
        .with_repo("acme", "widget", "main")
        .with_tree(
            "acme",
            "widget",
            tree("t1", false, vec![blob_entry("big.bin", "b1")]),
        )
        .with_blob(BlobResponse {
            sha: "b1".to_string(),
            size: 999_999,
            encoding: "base64".to_string(),
            content: "@@@ not valid base64 @@@".to_string(),
        });

    let result = materialize(
        &src,
        request("acme/widget", vec![selector("big", "big.bin", Some(10))]),
    )
    .await;

    let art = &result.repos[0].artifacts[0];
    assert_eq!(art.state, ArtifactState::TooLarge);
    assert_eq!(art.size_bytes, Some(999_999));
    assert_eq!(art.content, None);
}

#[tokio::test]
async fn oversized_decoded_blob_is_too_large() {
    // Declared size is within the limit but the decoded bytes exceed it.
    let big = "x".repeat(100);
    let src = FakeSource::default()
        .with_repo("acme", "widget", "main")
        .with_tree(
            "acme",
            "widget",
            tree("t1", false, vec![blob_entry("data.txt", "b1")]),
        )
        .with_blob(bytes_blob("b1", big.as_bytes(), 5));

    let result = materialize(
        &src,
        request("acme/widget", vec![selector("data", "data.txt", Some(10))]),
    )
    .await;

    let art = &result.repos[0].artifacts[0];
    assert_eq!(art.state, ArtifactState::TooLarge);
    assert_eq!(art.content, None);
}

#[tokio::test]
async fn invalid_base64_is_typed_failure_without_content() {
    let src = FakeSource::default()
        .with_repo("acme", "widget", "main")
        .with_tree(
            "acme",
            "widget",
            tree("t1", false, vec![blob_entry("weird.txt", "b1")]),
        )
        .with_blob(BlobResponse {
            sha: "b1".to_string(),
            size: 8,
            encoding: "base64".to_string(),
            content: "@@@@@@@@".to_string(),
        });

    let result = materialize(
        &src,
        request("acme/widget", vec![selector("weird", "weird.txt", None)]),
    )
    .await;

    let art = &result.repos[0].artifacts[0];
    assert!(matches!(
        art.state,
        ArtifactState::Error | ArtifactState::Unsupported
    ));
    assert_eq!(art.content, None);
}

#[tokio::test]
async fn non_utf8_bytes_are_binary_without_content() {
    let src = FakeSource::default()
        .with_repo("acme", "widget", "main")
        .with_tree(
            "acme",
            "widget",
            tree("t1", false, vec![blob_entry("logo.png", "b1")]),
        )
        .with_blob(bytes_blob("b1", &[0xff, 0xfe, 0x00, 0x01], 4));

    let result = materialize(
        &src,
        request("acme/widget", vec![selector("logo", "logo.png", None)]),
    )
    .await;

    let art = &result.repos[0].artifacts[0];
    assert_eq!(art.state, ArtifactState::Binary);
    assert_eq!(art.content, None);
}

#[tokio::test]
async fn output_is_lexically_stable_across_repos_and_paths() {
    let src = FakeSource::default()
        .with_repo("acme", "beta", "main")
        .with_repo("acme", "alpha", "main")
        .with_tree(
            "acme",
            "beta",
            tree("tb", false, vec![blob_entry("b.txt", "bb")]),
        )
        .with_tree(
            "acme",
            "alpha",
            tree(
                "ta",
                false,
                vec![blob_entry("y.txt", "by"), blob_entry("x.txt", "bx")],
            ),
        )
        .with_blob(text_blob("bb", "b"))
        .with_blob(text_blob("by", "y"))
        .with_blob(text_blob("bx", "x"));

    let req = MaterializeRequest {
        contract_version: 1,
        repos: vec![
            RepoRequest {
                repo: "acme/beta".to_string(),
                selectors: vec![selector("all", "*.txt", None)],
            },
            RepoRequest {
                repo: "acme/alpha".to_string(),
                selectors: vec![selector("all", "*.txt", None)],
            },
        ],
    };

    let result = materialize(&src, req).await;

    let repos: Vec<_> = result.repos.iter().map(|r| r.repo.as_str()).collect();
    assert_eq!(repos, vec!["acme/alpha", "acme/beta"]);

    let alpha_paths: Vec<_> = result.repos[0]
        .artifacts
        .iter()
        .map(|a| a.path.as_deref().unwrap())
        .collect();
    assert_eq!(alpha_paths, vec!["x.txt", "y.txt"]);
}

#[tokio::test]
async fn materialize_clamps_selector_max_bytes_to_hard_ceiling() {
    // materialize() is a public entrypoint that callers can invoke without
    // going through validate_request() first. A selector that claims a
    // max_bytes above MAX_FILE_BYTES must still be clamped to the hard
    // ceiling, not honored as a raised limit.
    let src = FakeSource::default()
        .with_repo("acme", "widget", "main")
        .with_tree(
            "acme",
            "widget",
            tree("t1", false, vec![blob_entry("big.txt", "b1")]),
        )
        .with_blob(bytes_blob(
            "b1",
            b"irrelevant, declared size drives the check",
            MAX_FILE_BYTES + 1,
        ));

    let result = materialize(
        &src,
        request(
            "acme/widget",
            vec![selector("big", "big.txt", Some(MAX_FILE_BYTES + 1))],
        ),
    )
    .await;

    let art = &result.repos[0].artifacts[0];
    assert_eq!(art.state, ArtifactState::TooLarge);
    assert_eq!(art.content, None);
}

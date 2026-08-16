//! Materialization request/result contract for nave dependency evidence.
//!
//! This crate defines the shared vocabulary between a materialization
//! *caller* (something that wants specific files out of one or more repos)
//! and a materialization *executor* (which walks a repo tree, matches
//! selectors, and fetches blob content). It contains **no** GitHub or
//! network code — only the request/result types, strict `serde` (de)
//! serialization, and request validation.
//!
//! # Determinism
//!
//! The wire form is deterministic: when a [`MaterializeRequest`] or
//! [`MaterializeResult`] is serialized, repositories are sorted lexically
//! by their `owner/name` identity, selectors within a repo are sorted
//! lexically by `id`, and artifacts within a repo result are sorted
//! lexically by `path` (with `selector_id` as a tiebreak for artifacts
//! that share a path, or that have no matched path at all). This makes
//! diffs of materialization output stable across runs regardless of the
//! order callers or executors happened to build their collections in.

use std::collections::HashSet;

use anyhow::Result;
use base64::Engine;
use globset::Glob;
use nave_github::{BlobResponse, Repo, TreeResponse};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

/// Version of the materialization request/result contract implemented by
/// this crate. Callers and executors should agree on this value; a mismatch
/// is a caller error to be surfaced by whatever transport carries the
/// request (out of scope for this crate).
pub const CONTRACT_VERSION: u32 = 1;

/// Maximum number of bytes of file content this contract will carry inline.
/// Selectors may not request a larger `max_bytes` than this.
pub const MAX_FILE_BYTES: u64 = 4_194_304;

/// A request to materialize specific paths out of one or more repositories.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterializeRequest {
    pub contract_version: u32,
    pub repos: Vec<RepoRequest>,
}

impl Serialize for MaterializeRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut repos = self.repos.clone();
        repos.sort_by(|a, b| a.repo.cmp(&b.repo));
        for repo in &mut repos {
            repo.selectors.sort_by(|a, b| a.id.cmp(&b.id));
        }

        let mut state = serializer.serialize_struct("MaterializeRequest", 2)?;
        state.serialize_field("contract_version", &self.contract_version)?;
        state.serialize_field("repos", &repos)?;
        state.end()
    }
}

/// One repository's worth of selectors within a [`MaterializeRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoRequest {
    /// Exact `owner/name` repository identity.
    pub repo: String,
    pub selectors: Vec<Selector>,
}

/// A single caller-owned selection of repo-root paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    /// Stable caller-owned identity, unique within the enclosing repo's
    /// selector list. Echoed back on each matching [`Artifact`] so callers
    /// can correlate results without depending on matched paths.
    pub id: String,
    /// A repo-root exact path or glob pattern, using Nave's existing
    /// `globset` semantics (see `nave_config::matcher::PathMatcher`).
    pub pattern: String,
    /// Maximum content size to inline for artifacts matched by this
    /// selector, in bytes. Must not exceed [`MAX_FILE_BYTES`]. `None`
    /// leaves the limit to the executor's default.
    pub max_bytes: Option<u64>,
}

/// The outcome of attempting to materialize the artifact(s) matched by a
/// selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactState {
    /// The path was found and its content (or a summary of it) is present.
    Found,
    /// No path in the tree matched the selector.
    Absent,
    /// The selector matched something that could not be resolved to a blob
    /// (e.g. a submodule pointer or symlink target outside the tree).
    Unresolved,
    /// The matched blob exceeded the selector's (or contract's) size limit.
    TooLarge,
    /// The matched blob is binary and was not inlined.
    Binary,
    /// The matched path exists but its kind is not supported for
    /// materialization (e.g. a directory).
    Unsupported,
    /// An error occurred while resolving or fetching the artifact; see
    /// `detail`.
    Error,
}

/// One materialized artifact: the outcome of a single selector match (or
/// non-match) against a repo tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    /// The [`Selector::id`] this artifact answers.
    pub selector_id: String,
    /// The repo-root path that matched, or `None` if the selector matched
    /// nothing in the tree.
    pub path: Option<String>,
    pub blob_sha: Option<String>,
    pub size_bytes: Option<u64>,
    pub state: ArtifactState,
    /// Content encoding, e.g. `"utf-8"` or `"base64"`, present only when
    /// `content` is present.
    pub encoding: Option<String>,
    /// Inlined content, present only for `state == Found` artifacts within
    /// the applicable size limit.
    pub content: Option<String>,
    /// Human-readable detail, typically populated for non-`Found` states.
    pub detail: Option<String>,
}

/// Materialized results for a single repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoResult {
    /// Exact `owner/name` repository identity.
    pub repo: String,
    pub ref_name: String,
    pub tree_sha: String,
    /// Whether the tree was walked to completion, or truncated (e.g. by an
    /// API pagination limit) before all selectors could be resolved.
    pub tree_complete: bool,
    pub artifacts: Vec<Artifact>,
}

/// The result of executing a [`MaterializeRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterializeResult {
    pub contract_version: u32,
    pub repos: Vec<RepoResult>,
}

impl MaterializeResult {
    /// Build a result, normalizing it into the contract's deterministic
    /// order: repos sorted lexically by `repo`, and each repo's artifacts
    /// sorted lexically by `path` (artifacts with no matched path sort
    /// after those with one, using `selector_id` as a tiebreak).
    #[must_use]
    pub fn new(contract_version: u32, mut repos: Vec<RepoResult>) -> Self {
        repos.sort_by(|a, b| a.repo.cmp(&b.repo));
        for repo in &mut repos {
            repo.artifacts.sort_by_key(sort_key);
        }
        Self {
            contract_version,
            repos,
        }
    }
}

fn sort_key(artifact: &Artifact) -> (bool, String, String) {
    (
        artifact.path.is_none(),
        artifact.path.clone().unwrap_or_default(),
        artifact.selector_id.clone(),
    )
}

impl Serialize for MaterializeResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let normalized = MaterializeResult::new(self.contract_version, self.repos.clone());

        let mut state = serializer.serialize_struct("MaterializeResult", 2)?;
        state.serialize_field("contract_version", &normalized.contract_version)?;
        state.serialize_field("repos", &normalized.repos)?;
        state.end()
    }
}

/// Errors that make a [`MaterializeRequest`] invalid.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("request must include at least one repo")]
    EmptyRepos,

    #[error("repo {repo:?} must include at least one selector")]
    EmptySelectors { repo: String },

    #[error("repo {repo:?} appears more than once in the request")]
    DuplicateRepo { repo: String },

    #[error("selector id {id:?} is used more than once in repo {repo:?}")]
    DuplicateSelectorId { repo: String, id: String },

    #[error("repo identity {repo:?} is not a valid `owner/name` string")]
    InvalidRepoIdentity { repo: String },

    #[error("selector {id:?} in repo {repo:?} has a path-traversal pattern: {pattern:?}")]
    PathTraversal {
        repo: String,
        id: String,
        pattern: String,
    },

    #[error("selector {id:?} in repo {repo:?} has an absolute-path pattern: {pattern:?}")]
    AbsolutePath {
        repo: String,
        id: String,
        pattern: String,
    },

    #[error(
        "selector {id:?} in repo {repo:?} has max_bytes {max_bytes} exceeding the contract limit of {MAX_FILE_BYTES}"
    )]
    MaxBytesExceeded {
        repo: String,
        id: String,
        max_bytes: u64,
    },

    #[error("selector {id:?} in repo {repo:?} has an invalid pattern {pattern:?}: {source}")]
    InvalidPattern {
        repo: String,
        id: String,
        pattern: String,
        #[source]
        source: globset::Error,
    },
}

/// Validate a [`MaterializeRequest`] against the contract's rules.
///
/// Rejects: empty repos, empty selectors for a repo, duplicate repo
/// identities, duplicate selector ids (unique within a repo), non-
/// `owner/name` repo identities, path-traversal patterns (any `..`
/// segment), absolute patterns (leading `/`), syntactically invalid glob
/// patterns, and any `max_bytes` above [`MAX_FILE_BYTES`].
///
/// # Errors
///
/// Returns the first [`ValidationError`] encountered, in repo-then-selector
/// order.
pub fn validate_request(request: &MaterializeRequest) -> Result<(), ValidationError> {
    if request.repos.is_empty() {
        return Err(ValidationError::EmptyRepos);
    }

    let mut seen_repos: HashSet<&str> = HashSet::new();

    for repo_request in &request.repos {
        if !seen_repos.insert(repo_request.repo.as_str()) {
            return Err(ValidationError::DuplicateRepo {
                repo: repo_request.repo.clone(),
            });
        }

        if !is_valid_repo_identity(&repo_request.repo) {
            return Err(ValidationError::InvalidRepoIdentity {
                repo: repo_request.repo.clone(),
            });
        }

        if repo_request.selectors.is_empty() {
            return Err(ValidationError::EmptySelectors {
                repo: repo_request.repo.clone(),
            });
        }

        let mut seen_selector_ids: HashSet<&str> = HashSet::new();

        for selector in &repo_request.selectors {
            if !seen_selector_ids.insert(selector.id.as_str()) {
                return Err(ValidationError::DuplicateSelectorId {
                    repo: repo_request.repo.clone(),
                    id: selector.id.clone(),
                });
            }

            if selector.pattern.starts_with('/') {
                return Err(ValidationError::AbsolutePath {
                    repo: repo_request.repo.clone(),
                    id: selector.id.clone(),
                    pattern: selector.pattern.clone(),
                });
            }

            if selector.pattern.split('/').any(|segment| segment == "..") {
                return Err(ValidationError::PathTraversal {
                    repo: repo_request.repo.clone(),
                    id: selector.id.clone(),
                    pattern: selector.pattern.clone(),
                });
            }

            if let Some(max_bytes) = selector.max_bytes
                && max_bytes > MAX_FILE_BYTES
            {
                return Err(ValidationError::MaxBytesExceeded {
                    repo: repo_request.repo.clone(),
                    id: selector.id.clone(),
                    max_bytes,
                });
            }

            if let Err(source) = Glob::new(&selector.pattern) {
                return Err(ValidationError::InvalidPattern {
                    repo: repo_request.repo.clone(),
                    id: selector.id.clone(),
                    pattern: selector.pattern.clone(),
                    source,
                });
            }
        }
    }

    Ok(())
}

fn is_valid_repo_identity(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !owner.is_empty() && !name.is_empty()
}

/// Read-only access to the Git objects a [`materialize`] run needs.
///
/// Every method is a single `GET`; nothing here mutates repository state.
/// Failures (missing repo, rate limiting, auth/visibility) surface as
/// `anyhow::Error` and are turned into typed [`ArtifactState`]s by the
/// materializer — never into a false [`ArtifactState::Absent`].
pub trait MaterializeSource {
    /// Resolve repository metadata (used for its `default_branch`).
    async fn repository(&self, owner: &str, repo: &str) -> Result<Repo>;

    /// Resolve `ref_name` to its commit, including the commit's OWN tree
    /// object SHA — the correct source for `RepoResult.tree_sha`. Never
    /// use `tree()`'s response for this (see its doc comment).
    async fn commit(
        &self,
        owner: &str,
        repo: &str,
        ref_name: &str,
    ) -> Result<nave_github::CommitResponse>;

    /// Fetch the recursive tree for `ref_name`, for file-listing only.
    ///
    /// The response's top-level `sha` field is NOT a tree object hash when
    /// `ref_name` is a branch/tag name (GitHub's Git Trees API resolves the
    /// ref through to its commit and echoes that commit's SHA back as
    /// `sha`, not the tree's own hash) — never use it as a tree identity;
    /// use `commit()`'s `commit.tree.sha` instead.
    async fn tree(&self, owner: &str, repo: &str, ref_name: &str) -> Result<TreeResponse>;

    /// Fetch a single blob by its object SHA.
    async fn blob(&self, owner: &str, repo: &str, sha: &str) -> Result<BlobResponse>;
}

impl MaterializeSource for nave_github::GithubClient {
    async fn repository(&self, owner: &str, repo: &str) -> Result<Repo> {
        self.get_repo(owner, repo).await
    }

    async fn commit(
        &self,
        owner: &str,
        repo: &str,
        ref_name: &str,
    ) -> Result<nave_github::CommitResponse> {
        self.get_commit(owner, repo, ref_name).await
    }

    async fn tree(&self, owner: &str, repo: &str, ref_name: &str) -> Result<TreeResponse> {
        self.get_tree_recursive(owner, repo, ref_name).await
    }

    async fn blob(&self, owner: &str, repo: &str, sha: &str) -> Result<BlobResponse> {
        self.get_blob(owner, repo, sha).await
    }
}

/// Materialize a [`MaterializeRequest`] against a read-only [`MaterializeSource`].
///
/// For each requested repo this resolves the default branch, walks the
/// recursive tree, matches each selector's glob against `blob` entries, and
/// fetches + bounds the matching blobs. Every outcome is a typed
/// [`ArtifactState`]; transport failures become [`ArtifactState::Error`], not
/// false absence. The returned [`MaterializeResult`] is normalized into the
/// contract's deterministic order (repos, then artifacts by path).
///
/// This function enforces only the per-repo/per-selector/per-file rules; it
/// does not apply request-level aggregate caps (those live at the CLI layer).
pub async fn materialize<S: MaterializeSource>(
    source: &S,
    request: MaterializeRequest,
) -> MaterializeResult {
    let mut repos = Vec::with_capacity(request.repos.len());
    for repo_request in &request.repos {
        repos.push(materialize_repo(source, repo_request).await);
    }
    MaterializeResult::new(request.contract_version, repos)
}

async fn materialize_repo<S: MaterializeSource>(
    source: &S,
    repo_request: &RepoRequest,
) -> RepoResult {
    let (owner, name) = match repo_request.repo.split_once('/') {
        Some((owner, name)) if !owner.is_empty() && !name.is_empty() => (owner, name),
        _ => return repo_error(repo_request, "", "", "invalid repo identity"),
    };

    // Resolve the default branch.
    let repo = match source.repository(owner, name).await {
        Ok(repo) => repo,
        Err(err) => {
            return repo_error(
                repo_request,
                "",
                "",
                &format!("failed to resolve repository: {err:#}"),
            );
        }
    };
    let ref_name = repo.default_branch.clone();

    // Resolve the commit at ref_name, for its REAL tree object SHA — never
    // the recursive-tree endpoint's own `sha` field (see `tree()`'s doc
    // comment: that field is the commit SHA, not a tree hash, when fetched
    // by branch name).
    let commit = match source.commit(owner, name, &ref_name).await {
        Ok(commit) => commit,
        Err(err) => {
            return repo_error(
                repo_request,
                &ref_name,
                "",
                &format!("failed to resolve commit: {err:#}"),
            );
        }
    };

    // Walk the recursive tree.
    let tree = match source.tree(owner, name, &ref_name).await {
        Ok(tree) => tree,
        Err(err) => {
            return repo_error(
                repo_request,
                &ref_name,
                &commit.commit.tree.sha,
                &format!("failed to fetch tree: {err:#}"),
            );
        }
    };
    let tree_complete = !tree.truncated;

    // Pre-collect the blob entries once so each selector reuses them.
    let blob_entries: Vec<(&str, &str)> = tree
        .tree
        .iter()
        .filter(|entry| entry.entry_type == "blob")
        .map(|entry| (entry.path.as_str(), entry.sha.as_str()))
        .collect();

    let mut artifacts = Vec::new();
    for selector in &repo_request.selectors {
        materialize_selector(
            source,
            owner,
            name,
            selector,
            &blob_entries,
            tree_complete,
            &mut artifacts,
        )
        .await;
    }

    RepoResult {
        repo: repo_request.repo.clone(),
        ref_name,
        tree_sha: commit.commit.tree.sha,
        tree_complete,
        artifacts,
    }
}

async fn materialize_selector<S: MaterializeSource>(
    source: &S,
    owner: &str,
    name: &str,
    selector: &Selector,
    blob_entries: &[(&str, &str)],
    tree_complete: bool,
    out: &mut Vec<Artifact>,
) {
    let matcher = match Glob::new(&selector.pattern) {
        Ok(glob) => glob.compile_matcher(),
        Err(err) => {
            out.push(nonmatch_artifact(
                selector,
                ArtifactState::Error,
                Some(format!("invalid selector pattern: {err}")),
            ));
            return;
        }
    };

    let mut hits: Vec<(&str, &str)> = blob_entries
        .iter()
        .copied()
        .filter(|(path, _)| matcher.is_match(path))
        .collect();
    hits.sort_by(|a, b| a.0.cmp(b.0));

    if hits.is_empty() {
        // Absence is only authoritative when the tree is complete.
        let state = if tree_complete {
            ArtifactState::Absent
        } else {
            ArtifactState::Unresolved
        };
        out.push(nonmatch_artifact(selector, state, None));
        return;
    }

    // Defense-in-depth hard ceiling: validate_request() rejects selectors that
    // raise the limit above MAX_FILE_BYTES, but materialize() is a public
    // entrypoint that must not trust callers to have validated first.
    let limit = selector
        .max_bytes
        .unwrap_or(MAX_FILE_BYTES)
        .min(MAX_FILE_BYTES);
    for (path, sha) in hits {
        out.push(materialize_blob(source, owner, name, selector, path, sha, limit).await);
    }
}

async fn materialize_blob<S: MaterializeSource>(
    source: &S,
    owner: &str,
    name: &str,
    selector: &Selector,
    path: &str,
    sha: &str,
    limit: u64,
) -> Artifact {
    let blob = match source.blob(owner, name, sha).await {
        Ok(blob) => blob,
        Err(err) => {
            return blob_artifact(
                selector,
                path,
                sha,
                None,
                ArtifactState::Error,
                None,
                None,
                Some(format!("failed to fetch blob: {err:#}")),
            );
        }
    };

    // Bound by the *declared* size before decoding anything.
    if blob.size > limit {
        return blob_artifact(
            selector,
            path,
            sha,
            Some(blob.size),
            ArtifactState::TooLarge,
            None,
            None,
            Some(format!("declared size {} exceeds limit {limit}", blob.size)),
        );
    }

    // Strip embedded newlines GitHub inserts into Base64 payloads.
    let cleaned: String = blob.content.split_whitespace().collect();
    let bytes = match base64::engine::general_purpose::STANDARD.decode(cleaned.as_bytes()) {
        Ok(bytes) => bytes,
        Err(err) => {
            return blob_artifact(
                selector,
                path,
                sha,
                Some(blob.size),
                ArtifactState::Error,
                None,
                None,
                Some(format!("invalid base64 content: {err}")),
            );
        }
    };

    let decoded_len = bytes.len() as u64;

    // Bound by the *decoded* size too — the API's declared size can lie.
    if decoded_len > limit {
        return blob_artifact(
            selector,
            path,
            sha,
            Some(decoded_len),
            ArtifactState::TooLarge,
            None,
            None,
            Some(format!("decoded size {decoded_len} exceeds limit {limit}")),
        );
    }

    match String::from_utf8(bytes) {
        Ok(text) => blob_artifact(
            selector,
            path,
            sha,
            Some(decoded_len),
            ArtifactState::Found,
            Some("utf-8".to_string()),
            Some(text),
            None,
        ),
        Err(_) => blob_artifact(
            selector,
            path,
            sha,
            Some(decoded_len),
            ArtifactState::Binary,
            None,
            None,
            Some("content is not valid utf-8".to_string()),
        ),
    }
}

fn repo_error(
    repo_request: &RepoRequest,
    ref_name: &str,
    tree_sha: &str,
    detail: &str,
) -> RepoResult {
    let artifacts = repo_request
        .selectors
        .iter()
        .map(|selector| nonmatch_artifact(selector, ArtifactState::Error, Some(detail.to_string())))
        .collect();
    RepoResult {
        repo: repo_request.repo.clone(),
        ref_name: ref_name.to_string(),
        tree_sha: tree_sha.to_string(),
        tree_complete: false,
        artifacts,
    }
}

fn nonmatch_artifact(
    selector: &Selector,
    state: ArtifactState,
    detail: Option<String>,
) -> Artifact {
    Artifact {
        selector_id: selector.id.clone(),
        path: None,
        blob_sha: None,
        size_bytes: None,
        state,
        encoding: None,
        content: None,
        detail,
    }
}

#[allow(clippy::too_many_arguments)]
fn blob_artifact(
    selector: &Selector,
    path: &str,
    sha: &str,
    size_bytes: Option<u64>,
    state: ArtifactState,
    encoding: Option<String>,
    content: Option<String>,
    detail: Option<String>,
) -> Artifact {
    Artifact {
        selector_id: selector.id.clone(),
        path: Some(path.to_string()),
        blob_sha: Some(sha.to_string()),
        size_bytes,
        state,
        encoding,
        content,
        detail,
    }
}

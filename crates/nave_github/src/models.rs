use serde::Deserialize;
use time::OffsetDateTime;

#[derive(Debug, Clone, Deserialize)]
pub struct Repo {
    pub name: String,
    pub full_name: String,
    pub default_branch: String,
    pub clone_url: String,
    pub fork: bool,
    pub archived: bool,
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub pushed_at: Option<OffsetDateTime>,
    pub owner: RepoOwner,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepoOwner {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TreeResponse {
    pub sha: String,
    pub tree: Vec<TreeEntry>,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    #[serde(rename = "type")]
    pub entry_type: String,
    pub sha: String,
}

/// Response shape for `GET /search/repositories`.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResponse {
    pub total_count: u64,
    pub items: Vec<Repo>,
}

/// Response shape for `GET /repos/{owner}/{repo}/git/blobs/{sha}`.
///
/// `size` is the blob's declared byte length (before Base64 decoding) and
/// `content` is Base64-encoded text (with `encoding == "base64"`) that may
/// contain embedded newlines per GitHub's Git Blob API.
#[derive(Debug, Clone, Deserialize)]
pub struct BlobResponse {
    pub sha: String,
    pub size: u64,
    pub encoding: String,
    pub content: String,
}

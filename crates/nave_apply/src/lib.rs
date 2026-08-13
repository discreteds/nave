//! Apply-mode git-mutation verb contract for `nave pen` (`capabilities`,
//! `branch`, `commit`, `push`, `reset`). No git/network code — only the
//! wire types, strict `serde` (de)serialization, and request validation.
//! Mirrors `nave_materialize`'s conventions: `deny_unknown_fields` on every
//! request type, deterministic (sorted) envelope serialization, versioned
//! protocol.

use serde::{Deserialize, Serialize, Serializer};

pub const PROTOCOL_VERSION: u32 = 1;
pub const APPLY_VERBS: &[&str] = &["branch", "commit", "push", "reset"];

// ---------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("protocol_version {0} is not supported (expected {PROTOCOL_VERSION})")]
    ProtocolVersionMismatch(u32),
    #[error("request must name at least one repo")]
    EmptyRepos,
    #[error("duplicate repo in request: {0}")]
    DuplicateRepo(String),
    #[error("repo identity must be owner/name: {0}")]
    InvalidRepoIdentity(String),
    #[error("invalid ref name: {0}")]
    InvalidRefName(String),
    #[error("invalid sha (must be 40 hex chars): {0}")]
    InvalidSha(String),
    #[error("invalid bound path: {0}")]
    InvalidPath(String),
}

pub fn validate_envelope_repos(protocol_version: u32, repos: &[String]) -> Result<(), ValidationError> {
    if protocol_version != PROTOCOL_VERSION {
        return Err(ValidationError::ProtocolVersionMismatch(protocol_version));
    }
    if repos.is_empty() {
        return Err(ValidationError::EmptyRepos);
    }
    let mut seen = std::collections::HashSet::new();
    for r in repos {
        if !seen.insert(r.as_str()) {
            return Err(ValidationError::DuplicateRepo(r.clone()));
        }
        let parts: Vec<&str> = r.split('/').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(ValidationError::InvalidRepoIdentity(r.clone()));
        }
    }
    Ok(())
}

pub fn validate_ref_name(name: &str) -> Result<(), ValidationError> {
    let bad = name.is_empty()
        || name.starts_with('/')
        || name.ends_with('/')
        || name.contains("//")
        || name.contains("..")
        || name.starts_with('-')
        || name
            .chars()
            .any(|c| c.is_control() || c == ' ' || c == '~' || c == '^' || c == ':' || c == '?' || c == '*' || c == '[');
    if bad {
        return Err(ValidationError::InvalidRefName(name.to_string()));
    }
    Ok(())
}

pub fn validate_hex_sha(sha: &str) -> Result<(), ValidationError> {
    if sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(ValidationError::InvalidSha(sha.to_string()))
    }
}

pub fn validate_bound_path(path: &str) -> Result<(), ValidationError> {
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.starts_with('-')
        || path.split('/').any(|seg| seg == "..")
        || path == ".git"
        || path.starts_with(".git/");
    if bad {
        return Err(ValidationError::InvalidPath(path.to_string()));
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Shared envelope state
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterState {
    Ok,
    Error,
}

// ---------------------------------------------------------------------
// capabilities
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct CapabilitiesResult {
    pub protocol_version: u32,
    pub verbs: Vec<String>,
    pub adapter_state: AdapterState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------
// branch
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchRepoRequest {
    pub repo: String,
    pub base_ref: String,
    pub expected_base_sha: String,
}

impl Serialize for BranchRepoRequest {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("BranchRepoRequest", 3)?;
        st.serialize_field("repo", &self.repo)?;
        st.serialize_field("base_ref", &self.base_ref)?;
        st.serialize_field("expected_base_sha", &self.expected_base_sha)?;
        st.end()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchEnvelope {
    pub protocol_version: u32,
    pub apply_ref: String,
    pub repos: Vec<BranchRepoRequest>,
}

impl Serialize for BranchEnvelope {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut repos = self.repos.clone();
        repos.sort_by(|a, b| a.repo.cmp(&b.repo));
        let mut st = s.serialize_struct("BranchEnvelope", 3)?;
        st.serialize_field("protocol_version", &self.protocol_version)?;
        st.serialize_field("apply_ref", &self.apply_ref)?;
        st.serialize_field("repos", &repos)?;
        st.end()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BranchState {
    Ok,
    StaleBase,
    Exists,
    MissingRef,
    NotACommit,
    UnknownRepo,
}

#[derive(Debug, Clone, Serialize)]
pub struct BranchRepoResult {
    pub repo: String,
    pub base_ref: String,
    pub expected_base_sha: String,
    pub observed_base_sha: String,
    pub apply_ref: String,
    pub state: BranchState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BranchResult {
    pub protocol_version: u32,
    pub adapter_state: AdapterState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub repos: Vec<BranchRepoResult>,
}

// ---------------------------------------------------------------------
// commit
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitRepoRequest {
    pub repo: String,
    pub paths: Vec<String>,
}

impl Serialize for CommitRepoRequest {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("CommitRepoRequest", 2)?;
        st.serialize_field("repo", &self.repo)?;
        st.serialize_field("paths", &self.paths)?;
        st.end()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitEnvelope {
    pub protocol_version: u32,
    pub repos: Vec<CommitRepoRequest>,
}

impl Serialize for CommitEnvelope {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut repos = self.repos.clone();
        repos.sort_by(|a, b| a.repo.cmp(&b.repo));
        let mut st = s.serialize_struct("CommitEnvelope", 2)?;
        st.serialize_field("protocol_version", &self.protocol_version)?;
        st.serialize_field("repos", &repos)?;
        st.end()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommitState {
    Ok,
    NothingToCommit,
    DirtyOutsideBounds,
    InvariantViolated,
    MissingClone,
    NoApplyState,
    UnknownRepo,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitRepoResult {
    pub repo: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_commit_sha: Option<String>,
    pub state: CommitState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitResult {
    pub protocol_version: u32,
    pub adapter_state: AdapterState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub repos: Vec<CommitRepoResult>,
}

// ---------------------------------------------------------------------
// push
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PushRepoRequest {
    pub repo: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PushEnvelope {
    pub protocol_version: u32,
    pub repos: Vec<PushRepoRequest>,
}

impl Serialize for PushEnvelope {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut repos = self.repos.clone();
        repos.sort_by(|a, b| a.repo.cmp(&b.repo));
        let mut st = s.serialize_struct("PushEnvelope", 2)?;
        st.serialize_field("protocol_version", &self.protocol_version)?;
        st.serialize_field("repos", &repos)?;
        st.end()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PushState {
    Ok,
    MissingBranch,
    Diverged,
    PushRejected,
    NoApplyState,
    UnknownRepo,
}

#[derive(Debug, Clone, Serialize)]
pub struct PushRepoResult {
    pub repo: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_commit_sha: Option<String>,
    pub state: PushState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PushResult {
    pub protocol_version: u32,
    pub adapter_state: AdapterState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub repos: Vec<PushRepoResult>,
}

// ---------------------------------------------------------------------
// reset
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResetRepoRequest {
    pub repo: String,
    #[serde(default)]
    pub expected_pushed_sha: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResetEnvelope {
    pub protocol_version: u32,
    pub repos: Vec<ResetRepoRequest>,
}

impl Serialize for ResetEnvelope {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut repos = self.repos.clone();
        repos.sort_by(|a, b| a.repo.cmp(&b.repo));
        let mut st = s.serialize_struct("ResetEnvelope", 2)?;
        st.serialize_field("protocol_version", &self.protocol_version)?;
        st.serialize_field("repos", &repos)?;
        st.end()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResetState {
    Ok,
    RemoteCasMismatch,
    MissingBranch,
    UnknownRepo,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResetRepoResult {
    pub repo: String,
    pub local_reset: bool,
    pub remote_deleted: bool,
    pub state: ResetState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResetResult {
    pub protocol_version: u32,
    pub adapter_state: AdapterState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub repos: Vec<ResetRepoResult>,
}

//! `nave materialize --request FILE [--json]`
//!
//! Reads a [`MaterializeRequest`] from a file (never from a shell argument),
//! validates it against the contract *and* the aggregate request caps, runs
//! the materializer against GitHub, then clamps the report to the aggregate
//! output caps before emitting it.
//!
//! JSON (`--json`) is the only machine contract. The human summary prints
//! counts and per-state tallies but never any file content. On any
//! parse/validation/cap failure the command exits non-zero with a short,
//! generic error that never echoes the request file's contents.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Args;

use nave_config::{NaveConfig, load_default};
use nave_materialize::{
    ArtifactState, MaterializeRequest, MaterializeResult, materialize, validate_request,
};

/// Largest request file we will read into memory, in bytes.
const MAX_REQUEST_BYTES: u64 = 1_048_576;
/// Maximum number of repositories a single request may name.
const MAX_REPOS: usize = 500;
/// Maximum number of selectors per repository.
const MAX_SELECTORS_PER_REPO: usize = 256;
/// Maximum number of `Found` artifacts a report may carry.
const MAX_FOUND_ARTIFACTS: usize = 20_000;
/// Maximum total decoded content bytes a report may carry.
const MAX_TOTAL_CONTENT_BYTES: u64 = 268_435_456;

#[derive(Args, Debug)]
pub(crate) struct MaterializeArgs {
    /// Path to a JSON materialize request. Request JSON is only ever read
    /// from this file, never from a command-line argument.
    #[arg(long)]
    pub request: PathBuf,
    /// Emit the `MaterializeResult` as JSON to stdout (the only machine
    /// contract). Without this flag, a human summary is printed and no file
    /// content is ever shown.
    #[arg(long)]
    pub json: bool,
}

pub(crate) async fn run(args: MaterializeArgs) -> Result<()> {
    // 1. Read + parse + validate + cap the request. Every failure here is
    //    reported with a generic message that never echoes request contents.
    let request = load_request(&args.request)?;

    // 2. Wire auth + client and run the materializer.
    let cfg: NaveConfig = load_default()?;
    let auth = nave_github::detect_auth(cfg.github.use_gh_cli).await;
    let client = nave_github::GithubClient::new(&cfg.github.api_base, auth)?;

    let mut result = materialize(&client, request).await;

    // 3. Deterministically clamp the report to the aggregate output caps.
    clamp_report(&mut result);

    // 4. Emit.
    if args.json {
        // Serialization normalizes into the contract's deterministic order.
        let json = serde_json::to_string(&result)?;
        println!("{json}");
    } else {
        print_summary(&result);
    }

    Ok(())
}

/// Read, parse, validate, and cap-check the request file. All failure paths
/// return a generic error that never includes request-file contents.
fn load_request(path: &std::path::Path) -> Result<MaterializeRequest> {
    // Fast-path metadata check, then a hard-bounded read so an over-cap file
    // can never be pulled fully into memory even if metadata lies.
    if let Ok(meta) = std::fs::metadata(path)
        && meta.len() > MAX_REQUEST_BYTES
    {
        bail!("request file exceeds the {MAX_REQUEST_BYTES}-byte limit");
    }

    let Ok(file) = std::fs::File::open(path) else {
        bail!("could not open request file {}", path.display());
    };
    let mut bounded = file.take(MAX_REQUEST_BYTES + 1);
    let mut bytes = Vec::new();
    if bounded.read_to_end(&mut bytes).is_err() {
        bail!("could not read request file {}", path.display());
    }
    if bytes.len() as u64 > MAX_REQUEST_BYTES {
        bail!("request file exceeds the {MAX_REQUEST_BYTES}-byte limit");
    }

    // Deliberately discard the parse error detail: it can echo request
    // contents (unknown field names, offending values).
    let Ok(request) = serde_json::from_slice::<MaterializeRequest>(&bytes) else {
        bail!("request file is not valid materialize-contract JSON");
    };

    // Contract validation. The ValidationError Display echoes request-derived
    // values, so we map it to a generic category message instead.
    if validate_request(&request).is_err() {
        bail!("request failed contract validation");
    }

    // Aggregate request caps (numeric caps validate_request does not cover).
    if request.repos.len() > MAX_REPOS {
        bail!("request names more than {MAX_REPOS} repositories");
    }
    for repo in &request.repos {
        if repo.selectors.len() > MAX_SELECTORS_PER_REPO {
            bail!("a repository names more than {MAX_SELECTORS_PER_REPO} selectors");
        }
    }

    Ok(request)
}

/// Clamp the report to the aggregate output caps, iterating in the report's
/// deterministic order. Once a cap would be exceeded, further `Found`
/// artifacts are converted to a typed outcome and their content is dropped —
/// never emitted as unmarked partial success.
fn clamp_report(result: &mut MaterializeResult) {
    // Normalize first so the clamp walks the same deterministic order the
    // wire form will present.
    let normalized =
        MaterializeResult::new(result.contract_version, std::mem::take(&mut result.repos));
    *result = normalized;

    let mut found_count: usize = 0;
    let mut total_bytes: u64 = 0;

    for repo in &mut result.repos {
        for artifact in &mut repo.artifacts {
            if artifact.state != ArtifactState::Found {
                continue;
            }
            let content_len = artifact.content.as_ref().map_or(0, |c| c.len() as u64);

            if found_count + 1 > MAX_FOUND_ARTIFACTS {
                // Count cap exceeded → Unresolved.
                artifact.state = ArtifactState::Unresolved;
                artifact.content = None;
                artifact.encoding = None;
                artifact.detail = Some(format!(
                    "dropped: report exceeded the {MAX_FOUND_ARTIFACTS}-artifact cap"
                ));
            } else if total_bytes + content_len > MAX_TOTAL_CONTENT_BYTES {
                // Byte cap exceeded → TooLarge.
                artifact.state = ArtifactState::TooLarge;
                artifact.content = None;
                artifact.encoding = None;
                artifact.detail = Some(format!(
                    "dropped: report exceeded the {MAX_TOTAL_CONTENT_BYTES}-byte content cap"
                ));
            } else {
                found_count += 1;
                total_bytes += content_len;
            }
        }
    }
}

/// Print a human summary: repo/artifact counts and per-state tallies. Never
/// prints any artifact `content`.
fn print_summary(result: &MaterializeResult) {
    let mut tallies: BTreeMap<String, usize> = BTreeMap::new();
    let mut artifact_count = 0usize;
    for repo in &result.repos {
        for artifact in &repo.artifacts {
            artifact_count += 1;
            *tallies
                .entry(state_label(artifact.state).to_string())
                .or_default() += 1;
        }
    }

    println!(
        "materialize: {} repo(s), {} artifact(s) (contract v{})",
        result.repos.len(),
        artifact_count,
        result.contract_version
    );
    for (state, count) in &tallies {
        println!("  {state}: {count}");
    }
}

fn state_label(state: ArtifactState) -> &'static str {
    match state {
        ArtifactState::Found => "found",
        ArtifactState::Absent => "absent",
        ArtifactState::Unresolved => "unresolved",
        ArtifactState::TooLarge => "too_large",
        ArtifactState::Binary => "binary",
        ArtifactState::Unsupported => "unsupported",
        ArtifactState::Error => "error",
    }
}

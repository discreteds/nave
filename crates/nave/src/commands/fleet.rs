use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;

use nave_config::{NaveConfig, Term, cache_root, load_default};
use nave_search::{SearchOptions, run_search};

#[derive(Debug, Args)]
pub(crate) struct FleetArgs {
    #[command(subcommand)]
    pub action: FleetAction,
}

#[derive(Debug, Subcommand)]
pub(crate) enum FleetAction {
    /// List repos in the cached fleet (discovery owned by nave, not pulse-gh).
    List(FleetListArgs),
}

#[derive(Debug, Args)]
pub(crate) struct FleetListArgs {
    /// Emit a JSON array of `{owner, name, default_branch}`, sorted by `(owner, name)`.
    #[arg(long)]
    pub json: bool,
    /// Resolve this search term against the fleet cache (same syntax as
    /// `nave search`) and emit only matching repos. `repo:`-scoped terms
    /// match identity from scan metadata (no pull required); other terms
    /// match tracked-file content and require `nave pull`.
    #[arg(long, value_name = "TERM")]
    pub term: Option<String>,
}

#[derive(Serialize)]
struct FleetRepo {
    owner: String,
    name: String,
    default_branch: String,
}

#[allow(clippy::unused_async)]
pub(crate) async fn run(args: FleetArgs) -> Result<()> {
    match args.action {
        FleetAction::List(list) => run_list(list).await,
    }
}

#[allow(clippy::unused_async)]
async fn run_list(args: FleetListArgs) -> Result<()> {
    let cfg: NaveConfig = load_default()?;
    let root = match cfg.cache.root.clone() {
        Some(r) => r,
        None => cache_root()?,
    };
    let fleet_root = root.join("fleet");
    if !fleet_root.exists() {
        anyhow::bail!(
            "no fleet cache at {} — run `nave scan` first",
            fleet_root.display()
        );
    }

    let mut repos: Vec<FleetRepo> = match &args.term {
        None => list_all_repos(&root, &fleet_root)?,
        Some(term_text) => {
            let term = Term::parse(term_text)
                .with_context(|| format!("parsing --term {term_text:?}"))?;
            if term.scope.as_deref() == Some("repo") {
                // Identity match against scan metadata — no pull required.
                list_all_repos(&root, &fleet_root)?
                    .into_iter()
                    .filter(|r| {
                        let identity = format!("{}/{}", r.owner, r.name);
                        term.needles
                            .iter()
                            .any(|n| identity == *n || r.name == *n)
                    })
                    .collect()
            } else {
                // Content term — reuse the search matcher (checkout-gated).
                let options = SearchOptions {
                    terms: vec![term],
                    match_preds: vec![],
                    ignore_case: false,
                    enrich_holes: false,
                };
                let report = run_search(&root, &cfg, &options)?;
                report
                    .repos
                    .iter()
                    .filter_map(|r| {
                        nave_config::cache::read_repo_meta(&root, &r.owner, &r.repo)
                            .ok()
                            .flatten()
                            .map(|meta| FleetRepo {
                                owner: meta.owner,
                                name: meta.name,
                                default_branch: meta.default_branch,
                            })
                    })
                    .collect()
            }
        }
    };
    repos.sort_by(|a, b| {
        (a.owner.as_str(), a.name.as_str()).cmp(&(b.owner.as_str(), b.name.as_str()))
    });
    // A term matching nothing is a valid empty resolution (the caller
    // fail-closes on it); an empty fleet cache without a term is an error.
    if args.term.is_none() && repos.is_empty() {
        anyhow::bail!("fleet cache is empty — run `nave scan` first");
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&repos)?);
    } else {
        for r in &repos {
            println!("{}/{} ({})", r.owner, r.name, r.default_branch);
        }
    }
    Ok(())
}

/// Walk the fleet cache and collect every repo with a `meta.toml`, in
/// filesystem order (caller sorts).
fn list_all_repos(root: &std::path::Path, fleet_root: &std::path::Path) -> Result<Vec<FleetRepo>> {
    let mut repos: Vec<FleetRepo> = Vec::new();
    for owner_entry in std::fs::read_dir(fleet_root)
        .with_context(|| format!("reading fleet cache {}", fleet_root.display()))?
    {
        let owner_entry = owner_entry?;
        if !owner_entry.file_type()?.is_dir() {
            continue;
        }
        let owner = owner_entry.file_name().to_string_lossy().into_owned();
        for repo_entry in std::fs::read_dir(owner_entry.path())? {
            let repo_entry = repo_entry?;
            if !repo_entry.file_type()?.is_dir() {
                continue;
            }
            let name = repo_entry.file_name().to_string_lossy().into_owned();
            if let Some(meta) = nave_config::cache::read_repo_meta(&root, &owner, &name)? {
                repos.push(FleetRepo {
                    owner: meta.owner,
                    name: meta.name,
                    default_branch: meta.default_branch,
                });
            }
        }
    }
    Ok(repos)
}

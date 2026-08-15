use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;

use nave_config::{NaveConfig, cache_root, load_default};

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

    let mut repos: Vec<FleetRepo> = Vec::new();
    for owner_entry in std::fs::read_dir(&fleet_root)
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
            let meta = nave_config::cache::read_repo_meta(&root, &owner, &name)?;
            if let Some(meta) = meta {
                repos.push(FleetRepo {
                    owner: meta.owner,
                    name: meta.name,
                    default_branch: meta.default_branch,
                });
            }
        }
    }
    repos.sort_by(|a, b| {
        (a.owner.as_str(), a.name.as_str()).cmp(&(b.owner.as_str(), b.name.as_str()))
    });
    if repos.is_empty() {
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

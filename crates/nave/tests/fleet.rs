//! End-to-end coverage for `nave fleet list --json` against a seeded fleet cache.

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nave-fleet-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_config(home: &std::path::Path, cache_root: &std::path::Path) {
    let cfg_dir = home.join(".config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let toml = format!(
        "[github]\nuse_gh_cli = false\nusername = \"acme\"\n[cache]\nroot = \"{}\"\n",
        cache_root.display()
    );
    std::fs::write(cfg_dir.join("nave.toml"), toml).unwrap();
}

fn seed_repo(cache: &std::path::Path, owner: &str, name: &str, default_branch: &str) {
    let dir = cache.join("fleet").join(owner).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("meta.toml"),
        format!(
            "owner = \"{owner}\"\nname = \"{name}\"\ndefault_branch = \"{default_branch}\"\n\
             clone_url = \"https://example.invalid/{owner}/{name}.git\"\n"
        ),
    )
    .unwrap();
}

#[test]
fn fleet_list_emits_sorted_json() {
    let home = temp_dir("list");
    let cache = temp_dir("cache");
    // Seed out of order to prove sorting.
    seed_repo(&cache, "acme", "zeta", "main");
    seed_repo(&cache, "acme", "alpha", "develop");
    write_config(&home, &cache);

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_nave"))
        .args(["fleet", "list", "--json"])
        .env("HOME", &home)
        .output()
        .expect("failed to execute nave");

    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&cache);

    assert!(
        output.status.success(),
        "fleet list failed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(
        parsed,
        serde_json::json!([
            {"owner": "acme", "name": "alpha", "default_branch": "develop"},
            {"owner": "acme", "name": "zeta", "default_branch": "main"},
        ])
    );
}

#[test]
fn fleet_list_plain_text_form() {
    let home = temp_dir("plain");
    let cache = temp_dir("plain-cache");
    seed_repo(&cache, "acme", "alpha", "develop");
    write_config(&home, &cache);

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_nave"))
        .args(["fleet", "list"])
        .env("HOME", &home)
        .output()
        .expect("failed to execute nave");

    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&cache);

    assert!(
        output.status.success(),
        "fleet list failed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, "acme/alpha (develop)\n");
}

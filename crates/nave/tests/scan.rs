//! End-to-end coverage for `nave scan` against a mock GitHub API, focused on
//! the fail-soft contract: a repo whose tree fetch errors (e.g. an empty repo
//! returning 409 "Git Repository is empty") must be skipped with a warning,
//! not abort the fleet scan.

use std::io::{Read, Write};

/// Unique temp dir for a test.
fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nave-scan-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write a `nave.toml` under `home/.config/` pointing GitHub at `api_base`,
/// with a local cache root and `use_gh_cli` off.
fn write_config(home: &std::path::Path, api_base: &str, cache_root: &std::path::Path) {
    let cfg_dir = home.join(".config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let toml = format!(
        "[github]\napi_base = \"{api_base}\"\nuse_gh_cli = false\nusername = \"acme\"\n\
         [cache]\nroot = \"{}\"\n",
        cache_root.display()
    );
    std::fs::write(cfg_dir.join("nave.toml"), toml).unwrap();
}

/// A minimal HTTP/1.1 responder for one scan run: a repo listing plus two
/// tree fetches (one healthy repo, one empty repo -> 409). Serves `count`
/// accept/respond cycles then exits. Routes on the request path.
fn spawn_mock_github(count: usize) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let repos_body = r#"[
      {"name":"good","full_name":"acme/good","default_branch":"main","clone_url":"https://example.invalid/acme/good.git","fork":false,"archived":false,"owner":{"login":"acme"}},
      {"name":"empty","full_name":"acme/empty","default_branch":"main","clone_url":"https://example.invalid/acme/empty.git","fork":false,"archived":false,"owner":{"login":"acme"}}
    ]"#;
    let tree_body =
        r#"{"sha":"treegood","truncated":false,"tree":[{"path":"pyproject.toml","type":"blob","sha":"blob1"}]}"#;
    let empty_body = r#"{"message":"Git Repository is empty.","documentation_url":"https://docs.github.com/rest"}"#;

    std::thread::spawn(move || {
        for _ in 0..count {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let first_line = req.lines().next().unwrap_or("");

            let (status, body) = if first_line.contains("/users/") {
                ("200 OK", repos_body)
            } else if first_line.contains("/git/trees/") && first_line.contains("/empty/") {
                ("409 Conflict", empty_body)
            } else if first_line.contains("/git/trees/") {
                ("200 OK", tree_body)
            } else {
                ("404 Not Found", "{}")
            };

            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });

    port
}

#[test]
fn scan_skips_empty_repo_instead_of_aborting() {
    let home = temp_dir("skip-empty");
    let cache = temp_dir("cache");
    let port = spawn_mock_github(3);
    write_config(&home, &format!("http://127.0.0.1:{port}"), &cache);

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_nave"))
        .args(["scan", "--no-interaction"])
        .env("HOME", &home)
        .output()
        .expect("failed to execute nave");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // The healthy repo must be cached (tracked pyproject.toml)...
    let good_meta = cache.join("fleet/acme/good/meta.toml");
    assert!(good_meta.exists(), "healthy repo not cached");
    // ...and the empty repo must not have produced a cache entry.
    let empty_dir = cache.join("fleet/acme/empty");
    assert!(
        !empty_dir.exists(),
        "empty repo should have been skipped, not cached"
    );

    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&cache);

    assert!(
        output.status.success(),
        "expected scan to succeed despite the empty repo; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("skipping repo") && stderr.contains("name=empty"),
        "expected a skip warning naming the empty repo; stderr:\n{stderr}"
    );
}

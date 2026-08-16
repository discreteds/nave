#[test]
fn cli_runs() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_nave"))
        .arg("--help")
        .output()
        .expect("failed to execute nave");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Usage") || stdout.contains("USAGE"),
        "unexpected output: {stdout}"
    );
}

#[test]
fn subcommands_listed() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_nave"))
        .arg("--help")
        .output()
        .expect("failed to execute nave");

    let stdout = String::from_utf8_lossy(&output.stdout);
    for sub in [
        "init",
        "scan",
        "pull",
        "check",
        "build",
        "search",
        "materialize",
    ] {
        assert!(
            stdout.contains(sub),
            "missing subcommand `{sub}` in help:\n{stdout}"
        );
    }
}

// -------------------------------------------------------------------------
// materialize command
// -------------------------------------------------------------------------

/// A distinctive string embedded in a request file; must never appear in the
/// command's output when the request is rejected.
const REQUEST_SECRET: &str = "SUPERSECRETcanaryTOKEN0xDEADBEEF";

/// Unique temp dir for a test, cleaned by the caller.
fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nave-materialize-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write a `nave.toml` under `home/.config/` pointing GitHub at `api_base`.
fn write_config(home: &std::path::Path, api_base: &str) {
    let cfg_dir = home.join(".config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let toml =
        format!("[github]\napi_base = \"{api_base}\"\nuse_gh_cli = false\nusername = \"x\"\n");
    std::fs::write(cfg_dir.join("nave.toml"), toml).unwrap();
}

/// A minimal HTTP/1.1 responder that answers the fixed sequence of GETs a
/// single-repo materialize run makes (repo, commit, tree, blob). Returns
/// the bound port. Runs `count` accept/respond cycles then exits.
fn spawn_mock_github(count: usize) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    // Canned bodies. The blob content is base64 for the ASCII text "hello\n".
    let repo_body = r#"{"name":"widget","full_name":"acme/widget","default_branch":"main","clone_url":"https://example.invalid/acme/widget.git","fork":false,"archived":false,"owner":{"login":"acme"}}"#;
    let commit_body = r#"{"sha":"commit123","commit":{"tree":{"sha":"tree123"}}}"#;
    let tree_body = r#"{"sha":"tree123","truncated":false,"tree":[{"path":"README.md","type":"blob","sha":"blobsha1"}]}"#;
    // base64("hello\n") == "aGVsbG8K"
    let blob_body = r#"{"sha":"blobsha1","size":6,"encoding":"base64","content":"aGVsbG8K"}"#;

    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for _ in 0..count {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            // Read just the request line (enough to route on the path).
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let first_line = req.lines().next().unwrap_or("");

            let body = if first_line.contains("/git/trees/") {
                tree_body
            } else if first_line.contains("/git/blobs/") {
                blob_body
            } else if first_line.contains("/commits/") {
                commit_body
            } else {
                // GET /repos/{owner}/{repo}
                repo_body
            };

            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });

    port
}

#[test]
fn materialize_invalid_request_exits_nonzero_without_echoing_contents() {
    let dir = temp_dir("invalid");
    // Bogus repo identity (no `owner/name`), carrying the secret canary.
    let request = format!(
        r#"{{"contract_version":1,"repos":[{{"repo":"{REQUEST_SECRET}","selectors":[{{"id":"s1","pattern":"README.md","max_bytes":null}}]}}]}}"#
    );
    let req_path = dir.join("request.json");
    std::fs::write(&req_path, &request).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_nave"))
        .arg("materialize")
        .arg("--request")
        .arg(&req_path)
        .arg("--json")
        .env("HOME", &dir)
        .output()
        .expect("failed to execute nave");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        !output.status.success(),
        "expected non-zero exit for invalid request"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains(REQUEST_SECRET),
        "secret leaked to stdout: {stdout}"
    );
    assert!(
        !stderr.contains(REQUEST_SECRET),
        "secret leaked to stderr: {stderr}"
    );
}

#[test]
fn materialize_json_output_validates() {
    let dir = temp_dir("json");
    let port = spawn_mock_github(4);
    write_config(&dir, &format!("http://127.0.0.1:{port}"));

    let request = r#"{"contract_version":1,"repos":[{"repo":"acme/widget","selectors":[{"id":"readme","pattern":"README.md","max_bytes":null}]}]}"#;
    let req_path = dir.join("request.json");
    std::fs::write(&req_path, request).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_nave"))
        .arg("materialize")
        .arg("--request")
        .arg(&req_path)
        .arg("--json")
        .env("HOME", &dir)
        .env_remove("NAVE_GITHUB_TOKEN")
        .output()
        .expect("failed to execute nave");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        output.status.success(),
        "expected success; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    assert_eq!(
        parsed
            .get("contract_version")
            .and_then(serde_json::Value::as_u64),
        Some(1),
        "missing/incorrect contract_version: {stdout}"
    );
    assert!(
        parsed.get("repos").is_some_and(serde_json::Value::is_array),
        "missing repos array: {stdout}"
    );
    // The matched blob decoded to "hello\n" and must be present in the report.
    let content = parsed["repos"][0]["artifacts"][0]["content"]
        .as_str()
        .unwrap_or("");
    assert!(
        content.contains("hello"),
        "expected materialized content in JSON: {stdout}"
    );
}

#[test]
fn materialize_human_output_omits_content() {
    let dir = temp_dir("human");
    let port = spawn_mock_github(4);
    write_config(&dir, &format!("http://127.0.0.1:{port}"));

    let request = r#"{"contract_version":1,"repos":[{"repo":"acme/widget","selectors":[{"id":"readme","pattern":"README.md","max_bytes":null}]}]}"#;
    let req_path = dir.join("request.json");
    std::fs::write(&req_path, request).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_nave"))
        .arg("materialize")
        .arg("--request")
        .arg(&req_path)
        .env("HOME", &dir)
        .env_remove("NAVE_GITHUB_TOKEN")
        .output()
        .expect("failed to execute nave");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        output.status.success(),
        "expected success; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The decoded file content ("hello") must never appear in human output.
    assert!(
        !stdout.contains("hello"),
        "human output leaked file content: {stdout}"
    );
    // But it should still report the found artifact somehow (counts/states).
    assert!(
        stdout.to_lowercase().contains("found") || stdout.contains('1'),
        "human output missing summary: {stdout}"
    );
}

#[test]
fn pull_without_cache_errors_cleanly() {
    let tmp = std::env::temp_dir().join(format!("nave-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_nave"))
        .arg("pull")
        .env("HOME", &tmp) // force cache lookup into empty dir
        .output()
        .expect("failed to execute nave");

    let _ = std::fs::remove_dir_all(&tmp);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not exist") || stderr.contains("run `nave scan`"),
        "unexpected stderr: {stderr}"
    );
}

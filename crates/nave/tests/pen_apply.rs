//! End-to-end proof: the real `nave` CLI binary, driven through
//! `capabilities → branch → commit → push → reset` against a real local
//! git remote, writing real request files and parsing real stdout JSON.
//! This is the "output actually lands on real clones" proof design spec
//! §7 requires.
//!
//! Config is pointed at the fixture's pen root via `NAVE_PEN__ROOT`
//! (figment's `Env::prefixed("NAVE_").split("__")` maps it to
//! `pen.root` — verified directly against the built binary; no
//! `nave.toml` file needed).

use std::process::Command;

fn nave(args: &[&str], pen_root: &std::path::Path) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_nave"))
        .args(args)
        .env("NAVE_PEN__ROOT", pen_root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "nave {args:?} exited {:?}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "bad json from nave {args:?}: {e}: {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

#[tokio::test]
// One linear end-to-end narrative through all five verbs — deliberately not split into
// smaller test functions, since the point is that each step's output feeds the next.
#[allow(clippy::too_many_lines)]
async fn full_apply_lifecycle_lands_on_real_clone_and_cleans_up() {
    let fx = nave_test_support::init_pen_fixture("e2e-apply", "acme", "docs", "develop").await;

    let caps = nave(&["pen", "capabilities", "--json"], fx.pen_root.path());
    assert_eq!(caps["verbs"].as_array().unwrap().len(), 4);

    let reqdir = tempfile::TempDir::new().unwrap();
    let branch_req_path = reqdir.path().join("branch.json");
    std::fs::write(
        &branch_req_path,
        format!(
            r#"{{"protocol_version":1,"apply_ref":"pulse/apply/e2e","repos":[{{"repo":"acme/docs","base_ref":"develop","expected_base_sha":"{}"}}]}}"#,
            fx.base_sha,
        ),
    )
    .unwrap();
    let branch_res = nave(
        &[
            "pen",
            "branch",
            "e2e-apply",
            "--request",
            branch_req_path.to_str().unwrap(),
            "--json",
        ],
        fx.pen_root.path(),
    );
    assert_eq!(branch_res["repos"][0]["state"], "ok");

    let dir = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), "e2e-apply", "acme", "docs");
    std::fs::write(dir.join("lockfile.json"), "{}").unwrap();

    let commit_req_path = reqdir.path().join("commit.json");
    std::fs::write(
        &commit_req_path,
        r#"{"protocol_version":1,"repos":[{"repo":"acme/docs","paths":["lockfile.json"]}]}"#,
    )
    .unwrap();
    let commit_res = nave(
        &[
            "pen",
            "commit",
            "e2e-apply",
            "pulse/apply/e2e",
            "--request",
            commit_req_path.to_str().unwrap(),
            "-m",
            "bump lockfile",
            "--json",
        ],
        fx.pen_root.path(),
    );
    assert_eq!(commit_res["repos"][0]["state"], "ok");
    let local_sha = commit_res["repos"][0]["local_commit_sha"]
        .as_str()
        .unwrap()
        .to_string();

    let push_req_path = reqdir.path().join("push.json");
    std::fs::write(
        &push_req_path,
        r#"{"protocol_version":1,"repos":[{"repo":"acme/docs"}]}"#,
    )
    .unwrap();
    let push_res = nave(
        &[
            "pen",
            "push",
            "e2e-apply",
            "pulse/apply/e2e",
            "--request",
            push_req_path.to_str().unwrap(),
            "--json",
        ],
        fx.pen_root.path(),
    );
    assert_eq!(push_res["repos"][0]["state"], "ok");
    assert_eq!(push_res["repos"][0]["remote_sha"], local_sha);

    // Confirm the branch actually landed on the real remote, not just that the CLI said so.
    let remote_ref = Command::new("git")
        .arg("-C")
        .arg(fx.origin.path())
        .args([
            "for-each-ref",
            "--format=%(objectname)",
            "refs/heads/pulse/apply/e2e",
        ])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&remote_ref.stdout).trim(),
        local_sha
    );

    let reset_req_path = reqdir.path().join("reset.json");
    std::fs::write(&reset_req_path, format!(r#"{{"protocol_version":1,"repos":[{{"repo":"acme/docs","expected_pushed_sha":"{local_sha}"}}]}}"#)).unwrap();
    let reset_res = nave(
        &[
            "pen",
            "reset",
            "e2e-apply",
            "pulse/apply/e2e",
            "--request",
            reset_req_path.to_str().unwrap(),
            "--json",
        ],
        fx.pen_root.path(),
    );
    assert_eq!(reset_res["repos"][0]["state"], "ok");
    assert_eq!(reset_res["repos"][0]["remote_deleted"], true);

    // Confirm the remote branch is really gone, not just reported gone.
    let remote_ref_after = Command::new("git")
        .arg("-C")
        .arg(fx.origin.path())
        .args(["for-each-ref", "refs/heads/pulse/apply/e2e"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&remote_ref_after.stdout)
            .trim()
            .is_empty()
    );
}

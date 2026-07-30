//! Contract tests for `nave_materialize`.
//!
//! These tests pin the request/result wire contract: strict deserialization
//! (unknown keys rejected), request validation rules, and deterministic
//! serialization ordering (repos, selectors, matched paths all lexically
//! sorted).

use nave_materialize::{
    Artifact, ArtifactState, MAX_FILE_BYTES, MaterializeRequest, MaterializeResult, RepoRequest,
    RepoResult, Selector, ValidationError, validate_request,
};

fn selector(id: &str, pattern: &str, max_bytes: Option<u64>) -> Selector {
    Selector {
        id: id.to_string(),
        pattern: pattern.to_string(),
        max_bytes,
    }
}

fn repo_request(repo: &str, selectors: Vec<Selector>) -> RepoRequest {
    RepoRequest {
        repo: repo.to_string(),
        selectors,
    }
}

#[test]
fn valid_exact_request_passes_validation() {
    let request = MaterializeRequest {
        contract_version: 1,
        repos: vec![repo_request(
            "hiivmind/hiivmind-pulse-gh",
            vec![selector("cargo-toml", "Cargo.toml", None)],
        )],
    };

    assert!(validate_request(&request).is_ok());
}

#[test]
fn valid_glob_request_passes_validation() {
    let request = MaterializeRequest {
        contract_version: 1,
        repos: vec![repo_request(
            "hiivmind/hiivmind-pulse-gh",
            vec![selector("rust-sources", "crates/**/*.rs", Some(1024))],
        )],
    };

    assert!(validate_request(&request).is_ok());
}

#[test]
fn duplicate_selector_ids_are_rejected() {
    let request = MaterializeRequest {
        contract_version: 1,
        repos: vec![repo_request(
            "hiivmind/hiivmind-pulse-gh",
            vec![
                selector("dup", "Cargo.toml", None),
                selector("dup", "README.md", None),
            ],
        )],
    };

    let err = validate_request(&request).unwrap_err();
    assert!(matches!(err, ValidationError::DuplicateSelectorId { .. }));
}

#[test]
fn path_traversal_pattern_is_rejected() {
    let request = MaterializeRequest {
        contract_version: 1,
        repos: vec![repo_request(
            "hiivmind/hiivmind-pulse-gh",
            vec![selector("bad", "../x", None)],
        )],
    };

    let err = validate_request(&request).unwrap_err();
    assert!(matches!(err, ValidationError::PathTraversal { .. }));
}

#[test]
fn absolute_path_pattern_is_rejected() {
    let request = MaterializeRequest {
        contract_version: 1,
        repos: vec![repo_request(
            "hiivmind/hiivmind-pulse-gh",
            vec![selector("bad", "/etc/passwd", None)],
        )],
    };

    let err = validate_request(&request).unwrap_err();
    assert!(matches!(err, ValidationError::AbsolutePath { .. }));
}

#[test]
fn max_bytes_above_limit_is_rejected() {
    let request = MaterializeRequest {
        contract_version: 1,
        repos: vec![repo_request(
            "hiivmind/hiivmind-pulse-gh",
            vec![selector("too-big", "Cargo.toml", Some(MAX_FILE_BYTES + 1))],
        )],
    };

    let err = validate_request(&request).unwrap_err();
    assert!(matches!(err, ValidationError::MaxBytesExceeded { .. }));
}

#[test]
fn empty_repos_are_rejected() {
    let request = MaterializeRequest {
        contract_version: 1,
        repos: vec![],
    };

    let err = validate_request(&request).unwrap_err();
    assert!(matches!(err, ValidationError::EmptyRepos));
}

#[test]
fn empty_selectors_for_a_repo_are_rejected() {
    let request = MaterializeRequest {
        contract_version: 1,
        repos: vec![repo_request("hiivmind/hiivmind-pulse-gh", vec![])],
    };

    let err = validate_request(&request).unwrap_err();
    assert!(matches!(err, ValidationError::EmptySelectors { .. }));
}

#[test]
fn duplicate_repo_identities_are_rejected() {
    let request = MaterializeRequest {
        contract_version: 1,
        repos: vec![
            repo_request(
                "hiivmind/hiivmind-pulse-gh",
                vec![selector("a", "Cargo.toml", None)],
            ),
            repo_request(
                "hiivmind/hiivmind-pulse-gh",
                vec![selector("b", "README.md", None)],
            ),
        ],
    };

    let err = validate_request(&request).unwrap_err();
    assert!(matches!(err, ValidationError::DuplicateRepo { .. }));
}

#[test]
fn non_owner_name_repo_identity_is_rejected() {
    let request = MaterializeRequest {
        contract_version: 1,
        repos: vec![repo_request(
            "not-owner-slash-name",
            vec![selector("a", "Cargo.toml", None)],
        )],
    };

    let err = validate_request(&request).unwrap_err();
    assert!(matches!(err, ValidationError::InvalidRepoIdentity { .. }));
}

#[test]
fn invalid_glob_syntax_is_rejected() {
    let request = MaterializeRequest {
        contract_version: 1,
        repos: vec![repo_request(
            "hiivmind/hiivmind-pulse-gh",
            vec![selector("bad-glob", "crates/[", None)],
        )],
    };

    let err = validate_request(&request).unwrap_err();
    assert!(matches!(err, ValidationError::InvalidPattern { .. }));
}

#[test]
fn unknown_keys_in_request_json_are_rejected() {
    let json = r#"{
        "contract_version": 1,
        "repos": [
            {
                "repo": "hiivmind/hiivmind-pulse-gh",
                "selectors": [
                    {"id": "a", "pattern": "Cargo.toml", "max_bytes": null, "unexpected": true}
                ]
            }
        ]
    }"#;

    let result: Result<MaterializeRequest, _> = serde_json::from_str(json);
    assert!(result.is_err(), "unknown field must be rejected");
}

#[test]
fn unknown_top_level_keys_are_rejected() {
    let json = r#"{
        "contract_version": 1,
        "repos": [],
        "extra": "nope"
    }"#;

    let result: Result<MaterializeRequest, _> = serde_json::from_str(json);
    assert!(result.is_err(), "unknown top-level field must be rejected");
}

#[test]
fn request_serialization_sorts_repos_and_selectors_lexically() {
    let request = MaterializeRequest {
        contract_version: 1,
        repos: vec![
            repo_request(
                "zzz-owner/zzz-repo",
                vec![
                    selector("z-selector", "b.txt", None),
                    selector("a-selector", "a.txt", None),
                ],
            ),
            repo_request("aaa-owner/aaa-repo", vec![selector("only", "x.txt", None)]),
        ],
    };

    let value: serde_json::Value = serde_json::to_value(&request).unwrap();
    let repos = value["repos"].as_array().unwrap();

    assert_eq!(repos[0]["repo"], "aaa-owner/aaa-repo");
    assert_eq!(repos[1]["repo"], "zzz-owner/zzz-repo");

    let second_repo_selectors = repos[1]["selectors"].as_array().unwrap();
    assert_eq!(second_repo_selectors[0]["id"], "a-selector");
    assert_eq!(second_repo_selectors[1]["id"], "z-selector");
}

fn artifact(selector_id: &str, path: Option<&str>, state: ArtifactState) -> Artifact {
    Artifact {
        selector_id: selector_id.to_string(),
        path: path.map(str::to_string),
        blob_sha: None,
        size_bytes: None,
        state,
        encoding: None,
        content: None,
        detail: None,
    }
}

#[test]
fn result_serialization_sorts_repos_and_matched_paths_lexically() {
    let result = MaterializeResult::new(
        1,
        vec![
            RepoResult {
                repo: "zzz-owner/zzz-repo".to_string(),
                ref_name: "main".to_string(),
                tree_sha: "deadbeef".to_string(),
                tree_complete: true,
                artifacts: vec![
                    artifact("sel-b", Some("z/file.txt"), ArtifactState::Found),
                    artifact("sel-a", Some("a/file.txt"), ArtifactState::Found),
                ],
            },
            RepoResult {
                repo: "aaa-owner/aaa-repo".to_string(),
                ref_name: "main".to_string(),
                tree_sha: "cafef00d".to_string(),
                tree_complete: true,
                artifacts: vec![artifact("only", Some("m.txt"), ArtifactState::Found)],
            },
        ],
    );

    let value = serde_json::to_value(&result).unwrap();
    let repos = value["repos"].as_array().unwrap();

    assert_eq!(repos[0]["repo"], "aaa-owner/aaa-repo");
    assert_eq!(repos[1]["repo"], "zzz-owner/zzz-repo");

    let second_repo_artifacts = repos[1]["artifacts"].as_array().unwrap();
    assert_eq!(second_repo_artifacts[0]["path"], "a/file.txt");
    assert_eq!(second_repo_artifacts[1]["path"], "z/file.txt");
}

#[test]
fn result_serialization_is_stable_regardless_of_construction_order() {
    let a = MaterializeResult::new(
        1,
        vec![RepoResult {
            repo: "hiivmind/hiivmind-pulse-gh".to_string(),
            ref_name: "main".to_string(),
            tree_sha: "sha1".to_string(),
            tree_complete: true,
            artifacts: vec![
                artifact("b", Some("b.txt"), ArtifactState::Found),
                artifact("a", Some("a.txt"), ArtifactState::Found),
            ],
        }],
    );

    let b = MaterializeResult::new(
        1,
        vec![RepoResult {
            repo: "hiivmind/hiivmind-pulse-gh".to_string(),
            ref_name: "main".to_string(),
            tree_sha: "sha1".to_string(),
            tree_complete: true,
            artifacts: vec![
                artifact("a", Some("a.txt"), ArtifactState::Found),
                artifact("b", Some("b.txt"), ArtifactState::Found),
            ],
        }],
    );

    let json_a = serde_json::to_string(&a).unwrap();
    let json_b = serde_json::to_string(&b).unwrap();
    assert_eq!(json_a, json_b);
}

#[test]
fn artifact_state_serializes_as_snake_case() {
    let value = serde_json::to_value(ArtifactState::TooLarge).unwrap();
    assert_eq!(value, serde_json::json!("too_large"));
}

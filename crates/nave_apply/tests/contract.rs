use nave_apply::{
    AdapterState, BranchEnvelope, BranchRepoRequest, BranchState, CommitEnvelope, CommitRepoRequest,
    PROTOCOL_VERSION, PushEnvelope, PushRepoRequest, ResetEnvelope, ResetRepoRequest, ValidationError,
    validate_bound_path, validate_envelope_repos, validate_hex_sha, validate_ref_name,
};

fn branch_req(repo: &str) -> BranchRepoRequest {
    BranchRepoRequest { repo: repo.into(), base_ref: "develop".into(), expected_base_sha: "a".repeat(40) }
}

#[test]
fn protocol_version_mismatch_is_rejected() {
    let err = validate_envelope_repos(2, &["acme/docs".into()]).unwrap_err();
    assert!(matches!(err, ValidationError::ProtocolVersionMismatch(2)));
}

#[test]
fn empty_repos_is_rejected() {
    assert!(matches!(validate_envelope_repos(PROTOCOL_VERSION, &[]), Err(ValidationError::EmptyRepos)));
}

#[test]
fn duplicate_repo_is_rejected() {
    let repos = vec!["acme/docs".to_string(), "acme/docs".to_string()];
    assert!(matches!(validate_envelope_repos(PROTOCOL_VERSION, &repos), Err(ValidationError::DuplicateRepo(_))));
}

#[test]
fn non_owner_name_identity_is_rejected() {
    let repos = vec!["docs".to_string()];
    assert!(matches!(validate_envelope_repos(PROTOCOL_VERSION, &repos), Err(ValidationError::InvalidRepoIdentity(_))));
}

#[test]
fn ref_name_with_parent_traversal_is_rejected() {
    assert!(validate_ref_name("pulse/../etc").is_err());
}

#[test]
fn ref_name_with_leading_slash_is_rejected() {
    assert!(validate_ref_name("/pulse/apply/p1").is_err());
}

#[test]
fn valid_ref_name_passes() {
    assert!(validate_ref_name("pulse/apply/p1").is_ok());
}

#[test]
fn sha_must_be_exactly_40_hex_chars() {
    assert!(validate_hex_sha(&"a".repeat(40)).is_ok());
    assert!(validate_hex_sha(&"a".repeat(39)).is_err());
    assert!(validate_hex_sha("not-hex-not-hex-not-hex-not-hex-not-hex").is_err());
}

#[test]
fn bound_path_rejects_traversal_and_absolute_and_git_dir() {
    assert!(validate_bound_path("../secret").is_err());
    assert!(validate_bound_path("/etc/passwd").is_err());
    assert!(validate_bound_path(".git/config").is_err());
    assert!(validate_bound_path("package-lock.json").is_ok());
}

#[test]
fn unknown_keys_in_branch_request_are_rejected() {
    let raw = r#"{"protocol_version":1,"apply_ref":"y","repos":[{"repo":"a/b","base_ref":"main","expected_base_sha":"x","extra":true}]}"#;
    assert!(serde_json::from_str::<BranchEnvelope>(raw).is_err());
}

#[test]
fn branch_state_serializes_kebab_case() {
    assert_eq!(serde_json::to_string(&BranchState::StaleBase).unwrap(), "\"stale-base\"");
}

#[test]
fn adapter_state_serializes_snake_case() {
    assert_eq!(serde_json::to_string(&AdapterState::Error).unwrap(), "\"error\"");
}

#[test]
fn branch_envelope_serializes_repos_sorted_regardless_of_construction_order() {
    let e1 = BranchEnvelope { protocol_version: 1, apply_ref: "pulse/apply/p1".into(), repos: vec![branch_req("z/z"), branch_req("a/a")] };
    let e2 = BranchEnvelope { protocol_version: 1, apply_ref: "pulse/apply/p1".into(), repos: vec![branch_req("a/a"), branch_req("z/z")] };
    assert_eq!(serde_json::to_string(&e1).unwrap(), serde_json::to_string(&e2).unwrap());
}

#[test]
fn commit_request_roundtrips_without_message_field() {
    let raw = r#"{"protocol_version":1,"repos":[{"repo":"a/b","paths":["x.json"]}]}"#;
    let env: CommitEnvelope = serde_json::from_str(raw).unwrap();
    assert_eq!(env.repos[0].paths, vec!["x.json".to_string()]);
}

#[test]
fn commit_envelope_serializes_repos_sorted() {
    let e1 = CommitEnvelope {
        protocol_version: 1,
        repos: vec![
            CommitRepoRequest { repo: "z/z".into(), paths: vec!["a".into()] },
            CommitRepoRequest { repo: "a/a".into(), paths: vec!["b".into()] },
        ],
    };
    let e2 = CommitEnvelope {
        protocol_version: 1,
        repos: vec![
            CommitRepoRequest { repo: "a/a".into(), paths: vec!["b".into()] },
            CommitRepoRequest { repo: "z/z".into(), paths: vec!["a".into()] },
        ],
    };
    assert_eq!(serde_json::to_string(&e1).unwrap(), serde_json::to_string(&e2).unwrap());
}

#[test]
fn push_request_roundtrips() {
    let raw = r#"{"protocol_version":1,"repos":[{"repo":"a/b"}]}"#;
    let env: PushEnvelope = serde_json::from_str(raw).unwrap();
    assert_eq!(env.repos[0].repo, "a/b");
}

#[test]
fn reset_request_defaults_missing_expected_pushed_sha_to_none() {
    let raw = r#"{"protocol_version":1,"repos":[{"repo":"a/b"}]}"#;
    let env: ResetEnvelope = serde_json::from_str(raw).unwrap();
    assert_eq!(env.repos[0].expected_pushed_sha, None);
}

#[test]
fn reset_request_carries_expected_pushed_sha_when_present() {
    let raw = format!(r#"{{"protocol_version":1,"repos":[{{"repo":"a/b","expected_pushed_sha":"{}"}}]}}"#, "a".repeat(40));
    let env: ResetEnvelope = serde_json::from_str(&raw).unwrap();
    assert_eq!(env.repos[0].expected_pushed_sha, Some("a".repeat(40)));
}

#[test]
fn result_envelopes_carry_top_level_reason_and_it_is_omitted_when_none() {
    let ok = nave_apply::BranchResult { protocol_version: 1, adapter_state: AdapterState::Ok, reason: None, repos: vec![] };
    let json = serde_json::to_string(&ok).unwrap();
    assert!(!json.contains("reason"));

    let err = nave_apply::BranchResult { protocol_version: 1, adapter_state: AdapterState::Error, reason: Some("bad".into()), repos: vec![] };
    let json = serde_json::to_string(&err).unwrap();
    assert!(json.contains("\"reason\":\"bad\""));
}

// Silence unused-import warnings for request types only exercised via serde in this file's
// round-trip tests (PushRepoRequest/ResetRepoRequest are constructed only through deserialize).
#[allow(dead_code)]
fn _use_types(_: PushRepoRequest, _: ResetRepoRequest) {}

use nave_pen::compute_repo_state;

#[tokio::test]
async fn clone_path_present_when_repo_cloned() {
    let fx = nave_test_support::init_pen_fixture("status-fx", "acme", "docs", "main").await;
    let cache = tempfile::TempDir::new().unwrap();
    let state = compute_repo_state(fx.pen_root.path(), cache.path(), &fx.pen, &fx.pen.repos[0])
        .await
        .unwrap();
    let expected = nave_pen::pen_repo_clone_dir(fx.pen_root.path(), &fx.pen.name, "acme", "docs");
    assert_eq!(
        state.clone_path.as_deref(),
        Some(expected.to_str().unwrap())
    );
}

#[tokio::test]
async fn clone_path_absent_when_repo_missing() {
    let pen_root = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let pen = nave_pen::Pen {
        name: "no-clone".into(),
        created_at: time::OffsetDateTime::now_utc(),
        branch: "nave/no-clone".into(),
        filter: nave_pen::PenFilter::default(),
        repos: vec![nave_pen::PenRepo {
            owner: "acme".into(),
            name: "docs".into(),
            default_branch: "main".into(),
            clone_url: "file:///dev/null".into(),
            synced_at: time::OffsetDateTime::now_utc(),
        }],
        ops: vec![],
    };
    let state = compute_repo_state(pen_root.path(), cache.path(), &pen, &pen.repos[0])
        .await
        .unwrap();
    assert_eq!(state.clone_path, None);
}

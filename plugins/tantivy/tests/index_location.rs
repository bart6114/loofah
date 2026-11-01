#[test]
fn cache_location_is_local_and_vault_specific() {
    let global = tempfile::tempdir().unwrap();
    let first_vault = tempfile::tempdir().unwrap();
    let second_vault = tempfile::tempdir().unwrap();
    let first = hypr_search_cache::Cache::new(global.path(), first_vault.path()).unwrap();
    let second = hypr_search_cache::Cache::new(global.path(), second_vault.path()).unwrap();
    first.initialize(&mut |_| {}).unwrap();
    assert!(first.status().ready);
    assert!(first.path().starts_with(global.path().join("search_index")));
    assert_ne!(first.path(), second.path());
    assert!(!first_vault.path().join("search_index").exists());
}

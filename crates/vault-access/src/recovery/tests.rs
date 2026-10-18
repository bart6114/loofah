use super::*;
use crate::Versions;
use serde_json::json;

fn destination(server: &str, user: &str, vault: &str) -> Destination {
    Destination {
        server_id: server.into(),
        user_id: user.into(),
        vault_id: vault.into(),
    }
}

fn pending(id: &str, time: u64, destination: Destination) -> PendingMutation {
    let mut call = Call::mutation(
        "session_save_note",
        json!({"session_id":"s1","markdown":"unsent"}),
        Versions::new(),
    );
    call.mutation_id = Some(id.into());
    PendingMutation {
        attempted: false,
        created_at_ms: time,
        destination,
        call,
        error: None,
    }
}

#[test]
fn restart_preserves_order_and_isolates_server_user_and_vault_destinations() {
    let root = tempfile::tempdir().unwrap();
    let spool = RecoverySpool::new(root.path()).unwrap();
    let own = destination("server", "me", "vault");
    for draft in [
        pending("last", 30, own.clone()),
        pending("first", 10, own.clone()),
        pending("other-server", 1, destination("other", "me", "vault")),
        pending("other-user", 2, destination("server", "other", "vault")),
        pending("other-vault", 3, destination("server", "me", "other")),
    ] {
        spool.save(&draft).unwrap();
    }
    let mut attempted = pending("middle", 20, own.clone());
    attempted.attempted = true;
    attempted.error = Some(Error::new("conflict", "Newer server version"));
    spool.save(&attempted).unwrap();
    std::fs::write(root.path().join(".tmp-unfinished"), "partial").unwrap();
    std::fs::create_dir(root.path().join("ignored.json")).unwrap();
    drop(spool);
    let spool = RecoverySpool::new(root.path()).unwrap();
    let drafts = spool.list(&own).unwrap();
    assert_eq!(
        drafts
            .iter()
            .map(|p| p.call.mutation_id.as_deref().unwrap())
            .collect::<Vec<_>>(),
        ["first", "middle", "last"]
    );
    assert!(drafts[1].attempted);
    assert_eq!(drafts[1].error.as_ref().unwrap().code, "conflict");
    spool.acknowledge("middle").unwrap();
    assert_eq!(spool.list(&own).unwrap().len(), 2);
    assert_eq!(
        spool
            .list(&destination("server", "other", "vault"))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn invalid_mutations_and_corrupt_spool_files_never_silently_discard_work() {
    let root = tempfile::tempdir().unwrap();
    let spool = RecoverySpool::new(root.path()).unwrap();
    let own = destination("server", "me", "vault");
    let mut draft = pending("valid", 1, own.clone());
    for id in [
        None,
        Some("../escape"),
        Some("/absolute"),
        Some(".hidden"),
        Some("a\\b"),
    ] {
        draft.call.mutation_id = id.map(str::to_string);
        assert_eq!(spool.save(&draft).unwrap_err().code, "invalid_request");
    }
    assert!(spool.acknowledge("../escape").is_err());
    spool.save(&pending("keep", 2, own.clone())).unwrap();
    std::fs::write(root.path().join("broken.json"), b"{broken").unwrap();
    assert_eq!(spool.list(&own).unwrap_err().code, "invalid_data");
    assert!(root.path().join("keep.json").exists());
    std::fs::remove_file(root.path().join("broken.json")).unwrap();
    assert_eq!(spool.list(&own).unwrap()[0].call.args["markdown"], "unsent");
}

use serde::Deserialize;
use vault_sync::{pairing::PendingPairing, remote::Environment};
use zeroize::Zeroizing;

#[derive(Deserialize)]
struct Fixture {
    sessions: Vec<Session>,
}
#[derive(Deserialize)]
struct Session {
    token: String,
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires two verified staging sessions with an enrolled approver"]
async fn password_authenticated_pairing_uses_the_authenticated_staging_mailbox() {
    let path = std::env::var("LOOFAH_SYNC_ENGINE_FIXTURE").unwrap();
    let bytes = Zeroizing::new(std::fs::read(path).unwrap());
    let fixture: Fixture = serde_json::from_slice(&bytes).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(45), async {
        let new_mac = PendingPairing::begin(Environment::Staging, &fixture.sessions[0].token, None)
            .await
            .unwrap();
        let code = new_mac.code();
        let trusted = PendingPairing::begin(
            Environment::Staging,
            &fixture.sessions[1].token,
            Some(&code),
        )
        .await
        .unwrap();
        let (new_mac, trusted) = tokio::join!(new_mac.connect(), trusted.connect());
        let mut new_mac = new_mac.unwrap();
        let mut trusted = trusted.unwrap();
        new_mac
            .send(b"exact pending device identity")
            .await
            .unwrap();
        assert_eq!(
            trusted.receive().await.unwrap().as_slice(),
            b"exact pending device identity"
        );
        trusted
            .send(b"encrypted key and checkpoint fixture")
            .await
            .unwrap();
        assert_eq!(
            new_mac.receive().await.unwrap().as_slice(),
            b"encrypted key and checkpoint fixture"
        );
        let (a, b) = tokio::join!(new_mac.close(), trusted.close());
        a.unwrap();
        b.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let pending = PendingPairing::begin(Environment::Staging, &fixture.sessions[0].token, None)
            .await
            .unwrap();
        let code = pending.code();
        let (plate, _) = code.split_once('-').unwrap();
        let wrong = format!("{plate}-incorrect-password-words");
        let other = PendingPairing::begin(
            Environment::Staging,
            &fixture.sessions[1].token,
            Some(&wrong),
        )
        .await
        .unwrap();
        let (a, b) = tokio::join!(pending.connect(), other.connect());
        assert!(a.is_err());
        assert!(b.is_err());
    })
    .await
    .unwrap();
}

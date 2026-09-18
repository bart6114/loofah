use libsodium_rs::crypto_sign::{self as sign, KeyPair};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Error, Result, crypto::VaultKey};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Challenge {
    pub id: Uuid,
    pub vault_id: Uuid,
    pub session_id: String,
    pub user_id: String,
    pub kind: Kind,
    pub device_id: Uuid,
    pub public_key: String,
    pub authority: String,
    pub generation: Uuid,
    pub nonce: String,
    pub expires_at: u64,
    pub consumed_at: Option<u64>,
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Enroll,
    Bind,
}

pub struct ExpectedIdentity<'a> {
    pub vault: Uuid,
    pub user: &'a str,
    pub session: &'a str,
    pub device: Uuid,
    pub public_key: &'a str,
    pub authority: &'a str,
    pub generation: Uuid,
    pub kind: Kind,
}

pub struct VerifiedChallenge(Challenge);

impl Challenge {
    /// Expected identity comes from this Mac or the authenticated pairing channel,
    /// never from the server challenge being checked.
    pub fn verify(self, expected: ExpectedIdentity<'_>, now_ms: u64) -> Result<VerifiedChallenge> {
        if self.vault_id != expected.vault
            || self.user_id != expected.user
            || self.session_id != expected.session
            || self.device_id != expected.device
            || self.public_key != expected.public_key
            || self.authority != expected.authority
            || self.generation != expected.generation
            || self.kind != expected.kind
            || self.consumed_at.is_some()
            || self.expires_at <= now_ms
            || self.expires_at > now_ms.saturating_add(300_000)
            || !valid_hex(&self.nonce, 64)
            || !valid_hex(&self.public_key, 64)
            || !valid_hex(&self.authority, 64)
        {
            return Err(Error::Authentication);
        }
        Ok(VerifiedChallenge(self))
    }

    pub fn payload(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(&[
            "loofah-enrollment-v1".to_owned(),
            match self.kind {
                Kind::Enroll => "enroll",
                Kind::Bind => "bind",
            }
            .to_owned(),
            self.vault_id.to_string(),
            self.user_id.clone(),
            self.session_id.clone(),
            self.id.to_string(),
            self.nonce.clone(),
            self.device_id.to_string(),
            self.public_key.clone(),
            self.authority.clone(),
            self.generation.to_string(),
            self.expires_at.to_string(),
        ])
        .map_err(|_| Error::Authentication)
    }
}

impl VerifiedChallenge {
    pub fn id(&self) -> Uuid {
        self.0.id
    }

    pub fn sign_device(&self, key: &KeyPair) -> Result<String> {
        if hex(key.public_key.as_bytes()) != self.0.public_key {
            return Err(Error::Authentication);
        }
        sign::sign_detached(&self.0.payload()?, &key.secret_key)
            .map(|signature| hex(&signature))
            .map_err(|_| Error::Authentication)
    }

    pub fn approve_enrollment(&self, root: &VaultKey) -> Result<String> {
        let authority = root.enrollment_authority()?;
        if self.0.kind != Kind::Enroll
            || root.vault() != self.0.vault_id
            || hex(authority.public_key.as_bytes()) != self.0.authority
        {
            return Err(Error::Authentication);
        }
        sign::sign_detached(&self.0.payload()?, &authority.secret_key)
            .map(|signature| hex(&signature))
            .map_err(|_| Error::Authentication)
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_payload_matches_the_worker_signing_vector() {
        use sha2::{Digest, Sha256};
        let vector: serde_json::Value = serde_json::from_str(include_str!(
            "../../../apps/cloud/tests/enrollment-vector.json"
        ))
        .unwrap();
        let challenge: Challenge = serde_json::from_value(vector["challenge"].clone()).unwrap();
        let payload = challenge.payload().unwrap();
        assert_eq!(
            hex(&Sha256::digest(&payload)),
            vector["payload_sha256"].as_str().unwrap()
        );
        let key = KeyPair::from_seed(&[7; 32]).unwrap();
        assert_eq!(hex(key.public_key.as_bytes()), challenge.public_key);
        assert_eq!(
            hex(&sign::sign_detached(&payload, &key.secret_key).unwrap()),
            vector["signature"].as_str().unwrap()
        );
    }

    #[test]
    fn exact_identity_and_recovery_generation_are_required_before_signing() {
        let root = VaultKey::generate(Uuid::new_v4()).unwrap();
        let device = KeyPair::generate().unwrap();
        let authority = root.enrollment_authority().unwrap();
        let challenge = Challenge {
            id: Uuid::new_v4(),
            vault_id: root.vault(),
            session_id: "session".into(),
            user_id: "user".into(),
            kind: Kind::Enroll,
            device_id: Uuid::new_v4(),
            public_key: hex(device.public_key.as_bytes()),
            authority: hex(authority.public_key.as_bytes()),
            generation: Uuid::new_v4(),
            nonce: "ab".repeat(32),
            expires_at: 200_000,
            consumed_at: None,
        };
        let expected = || ExpectedIdentity {
            vault: challenge.vault_id,
            user: "user",
            session: "session",
            device: challenge.device_id,
            public_key: &challenge.public_key,
            authority: &challenge.authority,
            generation: challenge.generation,
            kind: Kind::Enroll,
        };
        let mut altered = challenge.clone();
        altered.device_id = Uuid::new_v4();
        assert!(altered.verify(expected(), 100_000).is_err());
        let mut altered = challenge.clone();
        altered.generation = Uuid::new_v4();
        assert!(altered.verify(expected(), 100_000).is_err());
        assert!(challenge.clone().verify(expected(), 300_000).is_err());
        let checked = challenge.clone().verify(expected(), 100_000).unwrap();
        assert_eq!(checked.sign_device(&device).unwrap().len(), 128);
        assert_eq!(checked.approve_enrollment(&root).unwrap().len(), 128);
        assert!(checked.sign_device(&authority).is_err());
    }
}

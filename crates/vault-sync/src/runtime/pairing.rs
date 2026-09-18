use super::*;
use crate::pairing::PendingPairing;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    challenge: Uuid,
    signature: String,
    kit: Vec<u8>,
    checkpoint: u64,
}
impl Drop for Approval {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.kit.zeroize();
    }
}

impl Runtime {
    pub(super) async fn start_pairing(&mut self, code: Option<String>) -> Result<(), String> {
        if self.pairing.is_some() {
            return Err("Cancel the current pairing first.".into());
        }
        let account = self.account.clone().ok_or("Connect your account first.")?;
        let authority = account
            .account
            .enrollment_authority
            .clone()
            .ok_or("Enroll the first device with a saved recovery kit.")?;
        let remote = self.remote.clone().ok_or("Connect your account first.")?;
        if code.is_some() && self.engine.is_none() {
            return Err("Only an enrolled device can approve pairing.".into());
        }
        if code.is_none() && self.engine.is_some() {
            return Err("This device is already enrolled.".into());
        }
        let credentials = self.credentials.clone();
        let credential =
            tokio::task::block_in_place(|| credentials.session(account.account.vault_id))
                .map_err(|e| e.to_string())?;
        self.phase("pairing");
        self.status.write().unwrap().pairing_role =
            Some(if code.is_some() { "approver" } else { "new" }.into());
        let pending = tokio::time::timeout(
            Duration::from_secs(20),
            PendingPairing::begin(
                Environment::Staging,
                std::str::from_utf8(&credential).map_err(|_| "Invalid account session.")?,
                code.as_deref(),
            ),
        )
        .await
        .map_err(|_| "Could not reach the pairing service. Try again.".to_owned())
        .and_then(|result| result.map_err(sync_error));
        let pending = match pending {
            Ok(pending) => pending,
            Err(error) => {
                self.status.write().unwrap().pairing_role = None;
                self.phase(self.connected_phase());
                return Err(error);
            }
        };
        if code.is_none() {
            self.status.write().unwrap().pairing_code = Some(pending.code());
        }
        // The scheduler waits for pairing without changing the user's saved pause preference.
        let checkpoint = self
            .engine
            .as_ref()
            .map(|engine| engine.cursor())
            .unwrap_or(0);
        let directory = self.root.join("replica");
        let enrollment_name = format!("pending-enrollment:{}", account.account.vault_id);
        self.phase("pairing");
        self.pairing = Some(tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(600), async move {
                let mut channel = pending.connect().await.map_err(|e| e.to_string())?;
                if code.is_some() {
                    let (key, _, _) = tokio::task::block_in_place(|| credentials.identity(account.account.vault_id)).map_err(|e| e.to_string())?;
                    let request = channel.receive().await.map_err(|e| e.to_string())?;
                    let challenge: Challenge = serde_json::from_slice(&request).map_err(|_| "Invalid pairing identity.")?;
                    if challenge.session_id == account.identity.session { return Err("Pairing requires a different account session.".into()); }
                    let verified = challenge.clone().verify(ExpectedIdentity { vault: account.account.vault_id, user: &account.identity.user, session: &challenge.session_id, device: challenge.device_id, public_key: &challenge.public_key, authority: &authority, generation: account.account.recovery_generation, kind: Kind::Enroll }, now_ms()).map_err(|e| e.to_string())?;
                    let signature = verified.approve_enrollment(&key).map_err(|e| e.to_string())?;
                    remote.post::<serde_json::Value>("/enrollment/approve-pairing", &serde_json::json!({ "challenge": verified.id(), "authoritySignature": signature })).await.map_err(|e| e.to_string())?;
                    let approval = Approval { challenge: verified.id(), signature, kit: key.recovery_kit().to_vec(), checkpoint };
                    let bytes = Zeroizing::new(serde_json::to_vec(&approval).map_err(|_| "Invalid pairing approval.")?);
                    channel.send(&bytes).await.map_err(|e| e.to_string())?;
                    if channel.receive().await.map_err(|e| e.to_string())?.as_slice() != verified.id().as_bytes() { return Err("Pairing acknowledgement failed.".into()); }
                    channel.close().await.map_err(|e| e.to_string())?;
                    Ok(None)
                } else {
                    let device = Uuid::new_v4();
                    let pair = KeyPair::generate().map_err(|_| "Device identity generation failed.")?;
                    let public_key = hex(pair.public_key.as_bytes());
                    let challenge: Challenge = remote.post("/enrollment/challenge", &serde_json::json!({ "kind": "enroll", "device": device, "publicKey": public_key, "authority": authority, "pairing": true })).await.map_err(|e| e.to_string())?;
                    let verified = challenge.clone().verify(ExpectedIdentity { vault: account.account.vault_id, user: &account.identity.user, session: &account.identity.session, device, public_key: &public_key, authority: &authority, generation: account.account.recovery_generation, kind: Kind::Enroll }, now_ms()).map_err(|e| e.to_string())?;
                    channel.send(&serde_json::to_vec(&challenge).map_err(|_| "Invalid pairing identity.")?).await.map_err(|e| e.to_string())?;
                    let received = channel.receive().await.map_err(|e| e.to_string())?;
                    let approval: Approval = serde_json::from_slice(&received).map_err(|_| "Invalid pairing approval.")?;
                    let key = VaultKey::from_recovery_kit(&approval.kit, account.account.vault_id).map_err(|e| e.to_string())?;
                    if approval.challenge != verified.id() || hex(key.enrollment_authority().map_err(|e| e.to_string())?.public_key.as_bytes()) != authority { return Err("Pairing authority did not match this vault.".into()); }
                    tokio::task::block_in_place(|| { credentials.write(&enrollment_name, &Zeroizing::new(serde_json::to_vec(&PendingEnrollment { challenge: challenge.clone(), authority_signature: approval.signature.clone() }).map_err(|_| crate::Error::Authentication)?))?; credentials.save_confirmed_identity(&key, device, &pair, &approval.kit)?; atomic_json(&directory.join("paired-checkpoint.json"), &approval.checkpoint) }).map_err(|e| e.to_string())?;
                    remote.post::<serde_json::Value>("/enrollment/finish", &serde_json::json!({ "challenge": verified.id(), "deviceSignature": verified.sign_device(&pair).map_err(|e| e.to_string())?, "authoritySignature": approval.signature })).await.map_err(|e| e.to_string())?;
                    channel.send(verified.id().as_bytes()).await.map_err(|e| e.to_string())?;
                    channel.close().await.map_err(|e| e.to_string())?;
                    Ok(Some((key, device, pair)))
                }
            }).await.map_err(|_| "Pairing expired. Create a new code.")?
        }));
        Ok(())
    }
}

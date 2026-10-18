use crate::{Call, Error, Reply, Result, VaultAccess};

#[derive(Clone)]
pub struct RemoteVault {
    pub client: reqwest::Client,
    pub base_url: String,
    pub vault_id: String,
    pub access_token: String,
}

impl VaultAccess for RemoteVault {
    async fn invoke(&self, call: Call) -> Result<Reply> {
        let response = self
            .client
            .post(format!(
                "{}/v1/vaults/{}/invoke",
                self.base_url.trim_end_matches('/'),
                self.vault_id
            ))
            .timeout(std::time::Duration::from_secs(30))
            .bearer_auth(&self.access_token)
            .json(&call)
            .send()
            .await
            .map_err(|error| Error::new("unavailable", error.to_string()))?;
        if !response.status().is_success() {
            return Err(response.json::<Error>().await.unwrap_or_else(|_| {
                Error::new("unavailable", "server returned an invalid error")
            }));
        }
        response
            .json()
            .await
            .map_err(|error| Error::new("invalid_data", error.to_string()))
    }
}

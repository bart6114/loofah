use crate::{Call, Error, Reply, Result, VaultAccess};

#[derive(Clone)]
pub struct RemoteVault {
    pub client: reqwest::Client,
    pub base_url: String,
    pub vault_id: String,
    pub access_token: String,
}

pub(crate) fn invoke_request(
    client: &reqwest::Client,
    base_url: &str,
    vault_id: &str,
    call: &Call,
) -> reqwest::RequestBuilder {
    client
        .post(format!(
            "{}/v1/vaults/{vault_id}/invoke",
            base_url.trim_end_matches('/')
        ))
        .timeout(std::time::Duration::from_secs(30))
        .json(call)
}

impl VaultAccess for RemoteVault {
    async fn invoke(&self, call: Call) -> Result<Reply> {
        let response = invoke_request(&self.client, &self.base_url, &self.vault_id, &call)
            .bearer_auth(&self.access_token)
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

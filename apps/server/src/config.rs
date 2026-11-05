use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, path::PathBuf};
use url::Url;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub storage_root: PathBuf,
    pub cache_root: PathBuf,
    pub public_url: Url,
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    pub oidc: OidcConfig,
    #[serde(default)]
    pub allow_insecure_loopback: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OidcConfig {
    pub issuer: String,
    pub audience: String,
    pub client_id: String,
    pub required_scope: String,
    #[serde(default)]
    pub login_scopes: Vec<String>,
    #[serde(default = "default_token_types")]
    pub access_token_types: Vec<String>,
}

fn default_listen() -> SocketAddr {
    "127.0.0.1:8027".parse().unwrap()
}
fn default_token_types() -> Vec<String> {
    vec!["at+jwt".into(), "application/at+jwt".into()]
}

impl Config {
    pub fn validate(&self) -> std::result::Result<(), String> {
        validate_url(&self.public_url, self.allow_insecure_loopback)?;
        let issuer = Url::parse(&self.oidc.issuer).map_err(|_| "Invalid OIDC issuer")?;
        validate_url(&issuer, self.allow_insecure_loopback)?;
        if !self.storage_root.is_absolute() || !self.cache_root.is_absolute() {
            return Err("Storage and cache roots must be absolute paths".into());
        }
        if self.oidc.audience.is_empty()
            || self.oidc.client_id.is_empty()
            || self.oidc.audience == self.oidc.client_id
        {
            return Err(
                "Use a dedicated API audience distinct from the native OAuth client ID".into(),
            );
        }
        if self.oidc.required_scope.is_empty()
            || self.oidc.required_scope.contains(char::is_whitespace)
        {
            return Err("Configure exactly one required API access scope".into());
        }
        if !self.oidc.login_scopes.is_empty()
            && (!self.oidc.login_scopes.iter().any(|scope| scope == "openid")
                || self.oidc.login_scopes.iter().any(|scope| {
                    scope.is_empty() || scope.len() > 1024 || scope.chars().any(char::is_whitespace)
                }))
        {
            return Err(
                "Login scopes must include openid and contain individual scope strings".into(),
            );
        }
        if self.oidc.access_token_types.is_empty()
            || self
                .oidc
                .access_token_types
                .iter()
                .any(|typ| !matches!(typ.as_str(), "at+jwt" | "application/at+jwt" | "JWT"))
        {
            return Err("Unsupported access-token type".into());
        }
        Ok(())
    }
}

pub(crate) fn validate_url(url: &Url, insecure: bool) -> std::result::Result<(), String> {
    if !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
        || url.host_str().is_none()
    {
        return Err("URLs must have a host and no credentials, query or fragment".into());
    }
    let loopback = url.host().is_some_and(|host| match host {
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
        _ => false,
    });
    if url.scheme() != "https" && !(insecure && loopback && url.scheme() == "http") {
        return Err(
            "HTTPS is required; insecure development URLs must use a literal loopback address"
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deployment_config_requires_https_dedicated_api_audience_and_scope() {
        let config: Config = serde_json::from_str(include_str!("../deploy/server.json")).unwrap();
        config.validate().unwrap();
        let mut insecure = config.clone();
        insecure.public_url = "http://company.example".parse().unwrap();
        assert!(insecure.validate().is_err());
        insecure.allow_insecure_loopback = true;
        assert!(insecure.validate().is_err());
        insecure.public_url = "http://127.0.0.1:8027".parse().unwrap();
        insecure.validate().unwrap();
        let mut confused = config.clone();
        confused.oidc.audience = confused.oidc.client_id.clone();
        assert!(confused.validate().is_err());
        let mut unscoped = config.clone();
        unscoped.oidc.required_scope.clear();
        assert!(unscoped.validate().is_err());
        let mut invalid_type = config;
        invalid_type.oidc.access_token_types = vec!["id_token".into()];
        assert!(invalid_type.validate().is_err());
    }
}

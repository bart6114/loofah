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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processing: Option<crate::processing::ProcessingConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcription: Option<TranscriptionConfig>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptionConfig {
    pub model: String,
    pub model_path: PathBuf,
    #[serde(default)]
    pub model_size_bytes: u64,
    #[serde(default)]
    pub model_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onnx: Option<OnnxConfig>,
    #[serde(default = "one_workload")]
    pub workload_groups: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub live_reserved_groups: usize,
    #[serde(default = "default_transcription_jobs")]
    pub max_jobs: usize,
    #[serde(default = "default_transcription_queue")]
    pub max_queued: usize,
    #[serde(default = "default_transcription_audio")]
    pub private_audio_bytes: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OnnxConfig {
    pub streaming_model_path: PathBuf,
    pub diarizer_model_path: PathBuf,
    pub snapshot_root: PathBuf,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

fn one_workload() -> usize {
    1
}
fn default_transcription_jobs() -> usize {
    256
}
fn default_transcription_queue() -> usize {
    16
}
fn default_transcription_audio() -> u64 {
    8 * 1024 * 1024 * 1024
}

impl TranscriptionConfig {
    pub(crate) fn model_fingerprint(&self) -> String {
        #[cfg(feature = "cpu-transcription")]
        if self.onnx.is_some() {
            return crate::transcription_onnx::model_fingerprint();
        }
        self.model_sha256.clone()
    }

    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.model.is_empty()
            || self.model.len() > 512
            || self.model.trim() != self.model
            || self.model.chars().any(char::is_control)
            || !self.model_path.is_absolute()
            || self.model_path.to_str().is_none()
            || match &self.onnx {
                None => {
                    self.model_size_bytes == 0
                        || self.model_size_bytes > 16 * 1024 * 1024 * 1024
                        || !hypr_loofah_protocol::is_transcription_sha256(&self.model_sha256)
                }
                Some(onnx) => {
                    self.model_size_bytes != 0
                        || !self.model_sha256.is_empty()
                        || [
                            &onnx.streaming_model_path,
                            &onnx.diarizer_model_path,
                            &onnx.snapshot_root,
                        ]
                        .iter()
                        .any(|path| !path.is_absolute() || path.to_str().is_none())
                        || !matches!(
                            (self.workload_groups, self.live_reserved_groups),
                            (1, 0) | (2, 1)
                        )
                }
            }
            || !(1..=8).contains(&self.workload_groups)
            || self.live_reserved_groups >= self.workload_groups
            || !(1..=256).contains(&self.max_jobs)
            || !(1..=16).contains(&self.max_queued)
            || self.private_audio_bytes == 0
            || self.private_audio_bytes > default_transcription_audio()
        {
            return Err("Invalid operator transcription model identity or capacity".into());
        }
        Ok(())
    }

    #[cfg(feature = "cpu-transcription")]
    pub(crate) fn limits(&self) -> hypr_loofah_core::transcription_jobs::TranscriptionLimits {
        hypr_loofah_core::transcription_jobs::TranscriptionLimits {
            max_jobs: self.max_jobs,
            max_queued: self.max_queued,
            private_audio_bytes: self.private_audio_bytes,
        }
    }
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
        if let Some(transcription) = &self.transcription {
            transcription.validate()?;
            if !cfg!(feature = "cpu-transcription") {
                return Err("This server build does not include CPU transcription".into());
            }
        }
        if let Some(processing) = &self.processing {
            processing.validate()?;
        }
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
    use serde_json::json;

    fn transcription() -> serde_json::Value {
        json!({"model":"operator-base","model_path":std::env::temp_dir().join("provisioned-model.bin"),"model_size_bytes":81768585,"model_sha256":"a".repeat(64)})
    }

    #[test]
    fn notes_configuration_has_no_implicit_transcription_model_or_capacity() {
        let config: Config = serde_json::from_str(include_str!("../deploy/server.json")).unwrap();
        assert!(config.transcription.is_none());
        assert!(
            serde_json::to_value(&config)
                .unwrap()
                .get("transcription")
                .is_none()
        );
        config.validate().unwrap();
    }

    #[test]
    fn operator_transcription_identity_and_capacity_are_strict_and_bounded_without_io() {
        let valid = transcription();
        let config: TranscriptionConfig = serde_json::from_value(valid.clone()).unwrap();
        config.validate().unwrap();
        assert_eq!(config.workload_groups, 1);
        assert_eq!(config.live_reserved_groups, 0);
        assert!(
            serde_json::to_value(&config)
                .unwrap()
                .get("live_reserved_groups")
                .is_none()
        );
        assert_eq!(config.max_jobs, 256);
        assert_eq!(config.max_queued, 16);
        assert_eq!(config.private_audio_bytes, 8 * 1024 * 1024 * 1024);
        for (key, value) in [
            ("model", json!("")),
            ("model", json!(" model")),
            ("model", json!("m\n")),
            ("model_path", json!("relative-model.bin")),
            ("model_size_bytes", json!(0)),
            ("model_size_bytes", json!(16u64 * 1024 * 1024 * 1024 + 1)),
            ("model_sha256", json!("A".repeat(64))),
            ("workload_groups", json!(0)),
            ("workload_groups", json!(9)),
            ("live_reserved_groups", json!(1)),
            ("max_jobs", json!(0)),
            ("max_jobs", json!(257)),
            ("max_queued", json!(0)),
            ("max_queued", json!(17)),
            ("private_audio_bytes", json!(0)),
            ("private_audio_bytes", json!(8u64 * 1024 * 1024 * 1024 + 1)),
        ] {
            let mut candidate = valid.clone();
            candidate[key] = value;
            assert!(
                serde_json::from_value::<TranscriptionConfig>(candidate)
                    .unwrap()
                    .validate()
                    .is_err(),
                "{key}"
            );
        }
        let mut reserved = valid.clone();
        reserved["workload_groups"] = json!(2);
        reserved["live_reserved_groups"] = json!(1);
        serde_json::from_value::<TranscriptionConfig>(reserved)
            .unwrap()
            .validate()
            .unwrap();
        for key in ["url", "api_key", "download", "future"] {
            let mut candidate = valid.clone();
            candidate[key] = json!("unsupported");
            assert!(serde_json::from_value::<TranscriptionConfig>(candidate).is_err());
        }
    }

    #[test]
    fn onnx_configuration_requires_explicit_paths_and_its_bounded_native_capacity() {
        let root = std::env::temp_dir();
        let value = json!({ "model":"speech", "model_path":root.join("batch"), "onnx":{
            "streaming_model_path":root.join("streaming"), "diarizer_model_path":root.join("diarizer"), "snapshot_root":root.join("snapshots")
        }});
        serde_json::from_value::<TranscriptionConfig>(value.clone())
            .unwrap()
            .validate()
            .unwrap();
        let mut reserved = value.clone();
        reserved["workload_groups"] = json!(2);
        reserved["live_reserved_groups"] = json!(1);
        serde_json::from_value::<TranscriptionConfig>(reserved.clone())
            .unwrap()
            .validate()
            .unwrap();
        reserved["workload_groups"] = json!(3);
        assert!(
            serde_json::from_value::<TranscriptionConfig>(reserved)
                .unwrap()
                .validate()
                .is_err()
        );
        for (field, changed) in [
            ("workload_groups", json!(2)),
            ("model_size_bytes", json!(3)),
            ("model_sha256", json!("a".repeat(64))),
        ] {
            let mut invalid = value.clone();
            invalid[field] = changed;
            assert!(
                serde_json::from_value::<TranscriptionConfig>(invalid)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        for field in [
            "streaming_model_path",
            "diarizer_model_path",
            "snapshot_root",
        ] {
            let mut invalid = value.clone();
            invalid["onnx"][field] = json!("relative");
            assert!(
                serde_json::from_value::<TranscriptionConfig>(invalid)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
    }

    #[cfg(not(feature = "cpu-transcription"))]
    #[test]
    fn configured_transcription_is_rejected_by_notes_only_build_instead_of_silently_ignored() {
        let mut config: Config =
            serde_json::from_str(include_str!("../deploy/server.json")).unwrap();
        config.transcription = Some(serde_json::from_value(transcription()).unwrap());
        assert!(
            config
                .validate()
                .unwrap_err()
                .contains("does not include CPU transcription")
        );
    }

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

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use base64::Engine;
use http_body_util::BodyExt;
use loofah_server::{Config, Server, http::router};
use openidconnect::{
    JsonWebKeyId, PrivateSigningKey,
    core::{CoreEdDsaPrivateSigningKey, CoreJwsSigningAlgorithm},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

async fn response(
    app: &axum::Router,
    method: &str,
    url: &str,
    body: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(url)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn oidc_signature_nonce_pkce_handoff_refresh_and_logout() {
    let provider = MockServer::start().await;
    let issuer = format!("{}/realm", provider.uri());
    // Deterministic test-only Ed25519 seed; it is never used by a deployed service.
    let mut der = vec![
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ];
    der.extend([7u8; 32]);
    let pem = format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----",
        base64::engine::general_purpose::STANDARD.encode(der)
    );
    let signer = CoreEdDsaPrivateSigningKey::from_ed25519_pem(
        &pem,
        Some(JsonWebKeyId::new("test-key".into())),
    )
    .unwrap();
    Mock::given(method("GET")).and(path("/realm/.well-known/openid-configuration")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"issuer":issuer,"authorization_endpoint":format!("{}/authorize",provider.uri()),"token_endpoint":format!("{}/token",provider.uri()),"jwks_uri":format!("{}/jwks",provider.uri()),"response_types_supported":["code"],"subject_types_supported":["public"],"id_token_signing_alg_values_supported":["EdDSA"],"code_challenge_methods_supported":["S256"]}))).mount(&provider).await;
    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"keys":[signer.as_verification_key()]})),
        )
        .mount(&provider)
        .await;
    let root = tempfile::tempdir().unwrap();
    let server = Server::open(Config {
        data_dir: root.path().into(),
        public_url: "http://127.0.0.1:8947".into(),
        issuer: issuer.clone(),
        client_id: "desktop".into(),
        client_secret: None,
        max_upload_bytes: 1024,
    })
    .unwrap();
    let app = router(server.clone());
    let verifier = "a".repeat(64);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    for failure in [
        "valid",
        "audience",
        "issuer",
        "nonce",
        "expired",
        "signature",
        "missing-token",
        "provider-error",
        "pkce",
    ] {
        let start=response(&app,"POST","/v1/auth/start",json!({"callback":"http://127.0.0.1:54321/native","challenge":challenge,"state":"native-state-123456789012345678901234567890"})).await;
        assert_eq!(start.status(), StatusCode::OK);
        let start = json_body(start).await;
        let url = url::Url::parse(start["authorization_url"].as_str().unwrap()).unwrap();
        let fields = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        let nonce = fields.get("nonce").unwrap().to_string();
        let state = fields.get("state").unwrap().to_string();
        assert_eq!(fields["code_challenge_method"], "S256");
        let now = chrono::Utc::now().timestamp();
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&json!({"alg":"EdDSA","kid":"test-key"})).unwrap());
        let mut claims = json!({"iss":issuer,"aud":"desktop","sub":"stable-subject","email":"person@example.com","name":"Person","iat":now,"exp":now+60,"nonce":nonce});
        match failure {
            "audience" => claims["aud"] = json!("another-app"),
            "issuer" => claims["iss"] = json!("https://attacker.invalid"),
            "nonce" => claims["nonce"] = json!("wrong-nonce"),
            "expired" => claims["exp"] = json!(now - 60),
            _ => {}
        }
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&claims).unwrap());
        let signed = format!("{header}.{payload}");
        let mut signature = signer
            .sign(&CoreJwsSigningAlgorithm::EdDsa, signed.as_bytes())
            .unwrap();
        if failure == "signature" {
            signature[0] ^= 1;
        }
        let jwt = format!(
            "{signed}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
        );
        let mut provider_response =
            json!({"access_token":"upstream-access","token_type":"Bearer","id_token":jwt});
        if failure == "missing-token" {
            provider_response
                .as_object_mut()
                .unwrap()
                .remove("id_token");
        }
        if failure == "provider-error" {
            provider_response = json!({"error":"invalid_grant"});
        }
        let _token = Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(if failure == "provider-error" {
                    400
                } else {
                    200
                })
                .set_body_json(provider_response),
            )
            .mount_as_scoped(&provider)
            .await;
        let callback = response(
            &app,
            "GET",
            &format!("/v1/auth/callback?code=provider-code&state={state}"),
            Value::Null,
        )
        .await;
        if !matches!(failure, "valid" | "pkce") {
            assert_eq!(callback.status(), StatusCode::UNAUTHORIZED, "{failure}");
            continue;
        }
        assert_eq!(callback.status(), StatusCode::SEE_OTHER);
        let destination =
            url::Url::parse(callback.headers()["location"].to_str().unwrap()).unwrap();
        let grant = destination
            .query_pairs()
            .find(|(key, _)| key == "code")
            .unwrap()
            .1
            .into_owned();
        assert_eq!(
            destination
                .query_pairs()
                .find(|(key, _)| key == "state")
                .unwrap()
                .1,
            "native-state-123456789012345678901234567890"
        );
        if failure == "pkce" {
            assert_eq!(
                response(
                    &app,
                    "POST",
                    "/v1/auth/exchange",
                    json!({"code":grant,"verifier":"wrong-verifier"})
                )
                .await
                .status(),
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                response(
                    &app,
                    "POST",
                    "/v1/auth/exchange",
                    json!({"code":grant,"verifier":verifier})
                )
                .await
                .status(),
                StatusCode::UNAUTHORIZED
            );
            continue;
        }
        let exchange = json!({"code":grant,"verifier":verifier});
        let tokens = response(&app, "POST", "/v1/auth/exchange", exchange.clone()).await;
        assert_eq!(tokens.status(), StatusCode::OK);
        let tokens = json_body(tokens).await;
        assert_eq!(
            response(&app, "POST", "/v1/auth/exchange", exchange)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let refresh = response(
            &app,
            "POST",
            "/v1/auth/refresh",
            json!({"refresh_token":tokens["refresh_token"]}),
        )
        .await;
        assert_eq!(refresh.status(), StatusCode::OK);
        let refreshed = json_body(refresh).await;
        assert_ne!(tokens["access_token"], refreshed["access_token"]);
        assert_eq!(
            response(
                &app,
                "POST",
                "/v1/auth/refresh",
                json!({"refresh_token":tokens["refresh_token"]})
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
        let logout = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/auth/logout")
                    .header(
                        "authorization",
                        format!("Bearer {}", refreshed["access_token"].as_str().unwrap()),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(logout.status(), StatusCode::OK);
        assert_eq!(
            response(
                &app,
                "POST",
                "/v1/auth/refresh",
                json!({"refresh_token":refreshed["refresh_token"]})
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(server.registry.read().await.users.len(), 1);
    assert_eq!(server.registry.read().await.vaults.len(), 1);
    for callback in [
        "https://attacker.example/callback",
        "http://127.0.0.1.attacker.example:1234/callback",
        "http://localhost:1234/callback",
        "http://127.0.0.1/callback",
        "http://user:password@127.0.0.1:1234/callback",
        "http://127.0.0.1:1234/callback?state=injected",
        "http://127.0.0.1:1234/callback#fragment",
    ] {
        assert_eq!(response(&app,"POST","/v1/auth/start",json!({"callback":callback,"challenge":challenge,"state":"native-state-123456789012345678901234567890"})).await.status(),StatusCode::BAD_REQUEST);
    }
    for proof in [
        ("a".repeat(42), "s".repeat(32)),
        ("a".repeat(43), "s".repeat(31)),
        ("a".repeat(43), "s".repeat(257)),
    ] {
        assert_eq!(response(&app,"POST","/v1/auth/start",json!({"callback":"http://127.0.0.1:1234/native","challenge":proof.0,"state":proof.1})).await.status(),StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        response(
            &app,
            "GET",
            "/v1/auth/callback?code=unused&state=unknown",
            Value::Null
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
}

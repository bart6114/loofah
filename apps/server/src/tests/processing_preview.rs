use super::processing::{Provider, prepare, terminal};
use super::processing_api::{input, shared};
use super::*;
use std::time::Duration;

async fn read_until(response: &mut reqwest::Response, marker: &str) -> String {
    tokio::time::timeout(Duration::from_secs(8), async {
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.unwrap() {
            bytes.extend_from_slice(&chunk);
            assert!(bytes.len() < 128 * 1024);
            if String::from_utf8_lossy(&bytes).contains(marker) {
                break;
            }
        }
        String::from_utf8(bytes).unwrap()
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn processing_preview_http_is_private_partial_and_cursor_independent_until_actual_completion()
{
    let provider = Provider::with_outputs(0, vec!["<think>PRIVATE-REASONING</think># Release\n- Ship Friday.\n<loofah-tags>[\"PRIVATE-TAG\"]</loofah-tags>".into()]).await;
    let fixture = Fixture::with_processing(Some(provider.config())).await;
    let (vault, _) = shared(&fixture).await;
    let context = fixture.service.vault(&vault.id).await.unwrap();
    prepare(&context).await;
    let baseline = context
        .read_document("note", &DocumentTarget::Summary)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = fixture.app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let http = reqwest::Client::new();
    let events = format!("{origin}/api/v1/vaults/{}/events", vault.id);
    let mut preview_events = http
        .get(&events)
        .bearer_auth(fixture.token("alice"))
        .header("x-loofah-api-major", "1")
        .header("x-loofah-job-previews", "1")
        .send()
        .await
        .unwrap();
    let mut old_events = http
        .get(&events)
        .bearer_auth(fixture.token("alice"))
        .header("x-loofah-api-major", "1")
        .send()
        .await
        .unwrap();
    let jobs = format!("{origin}/api/v1/vaults/{}/jobs", vault.id);
    let job: ProcessingJobSnapshot = http
        .post(&jobs)
        .bearer_auth(fixture.token("alice"))
        .header("x-loofah-api-major", "1")
        .header("x-request-id", uuid::Uuid::new_v4().to_string())
        .json(&input(&context).await)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let preview_url = format!("{jobs}/{}/preview", job.id);
    let value: ProcessingJobPreview = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let response = http
                .get(&preview_url)
                .bearer_auth(fixture.token("alice"))
                .header("x-loofah-api-major", "1")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["cache-control"], "no-store");
            let value: ProcessingJobPreview = response.json().await.unwrap();
            if value
                .preview
                .as_ref()
                .is_some_and(|preview| !preview.markdown.is_empty())
            {
                break value;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(value.vault_id, vault.id);
    assert_eq!(value.job.id, job.id);
    assert_eq!(value.job.state, ProcessingJobState::Running);
    assert_eq!(
        value.preview.unwrap().markdown.trim(),
        "# Release\n- Ship Friday."
    );
    assert_eq!(
        context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap(),
        baseline
    );
    let denied = http
        .get(&preview_url)
        .bearer_auth(fixture.token("bob"))
        .header("x-loofah-api-major", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    let events = read_until(&mut preview_events, "event: processing_preview").await;
    let private = events
        .split("\n\n")
        .find(|entry| entry.contains("event: processing_preview"))
        .unwrap();
    assert!(!private.lines().any(|line| line.starts_with("id:")));
    assert!(!private.contains("Ship Friday") && !private.contains("PRIVATE-"));
    let sentinel = context.events.publish(Change::SearchReady).cursor;
    let old = read_until(&mut old_events, &sentinel).await;
    assert!(!old.contains("processing_preview") && !old.contains("job_preview_changed"));
    provider.finish.add_permits(1);
    assert_eq!(
        terminal(&context, &job.id).await.state,
        ProcessingJobState::Succeeded
    );
    let retained: ProcessingJobPreview = http
        .get(&preview_url)
        .bearer_auth(fixture.token("alice"))
        .header("x-loofah-api-major", "1")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(retained.job.state, ProcessingJobState::Succeeded);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    server.abort();
    let _ = server.await;
    fixture.service.shutdown().await.unwrap();
    provider.close().await;
}

use super::*;
use hypr_loofah_core::jobs::generation::SummaryImage;

fn input() -> ModelRequest {
    ModelRequest {
        system: "Summarize the meeting.".into(),
        prompt: "Alice will send the checklist.".into(),
        images: vec![],
        max_output_tokens: 8192,
        max_retries: 4,
    }
}

#[test]
fn image_input_is_bounded_and_encoded_without_local_paths() {
    let mut input = input();
    input.images.push(SummaryImage {
        bytes: vec![1, 2, 3],
        mime_type: "image/png".into(),
    });
    let request = prepare_request("exact-model-id".into(), input.clone()).unwrap();
    assert_eq!(request.model, "exact-model-id");
    assert_eq!(request.images, ["data:image/png;base64,AQID"]);
    assert_eq!(request.max_output_tokens, Some(8192));
    input.images[0].mime_type = "file:///private/image.png".into();
    assert!(matches!(
        prepare_request("m".into(), input.clone()),
        Err(ProcessingJobFailure::ResourceLimit)
    ));
    input.images[0].mime_type = "image/png".into();
    input.images[0].bytes.resize(128 * 1024 + 1, 0);
    assert!(matches!(
        prepare_request("m".into(), input),
        Err(ProcessingJobFailure::ResourceLimit)
    ));
}

#[test]
fn runtime_failures_are_sanitized_and_never_request_an_automatic_retry() {
    for (code, expected) in [
        ("cancelled", ProcessingJobFailure::Interrupted),
        ("runtime", ProcessingJobFailure::Interrupted),
        ("timeout", ProcessingJobFailure::Interrupted),
        ("keychain", ProcessingJobFailure::ProviderUnavailable),
        ("authentication", ProcessingJobFailure::ProviderUnavailable),
        ("quota", ProcessingJobFailure::ProviderFailed),
        ("resource_limit", ProcessingJobFailure::ResourceLimit),
    ] {
        assert_eq!(map_error(failure(code, "sk-private-payload")), expected);
    }
}

#[test]
fn account_signout_cancels_current_shared_jobs_without_poisoning_a_later_login() {
    let state = ChatgptState::default();
    let first = state.shared_cancel.lock().unwrap().clone();
    assert!(!first.is_cancelled());
    state.cancel_jobs();
    assert!(first.is_cancelled());
    let second = state.shared_cancel.lock().unwrap().clone();
    assert!(!second.is_cancelled());
    state.cancel_jobs();
    assert!(second.is_cancelled());
    assert!(!state.shared_cancel.lock().unwrap().is_cancelled());
}

#[cfg(unix)]
mod runtime {
    use super::*;
    use futures_util::{StreamExt, poll};

    fn request() -> ChatgptGeneration {
        prepare_request("model".into(), input()).unwrap()
    }

    async fn notification(events: &mut broadcast::Receiver<Value>, method: &str) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if events.recv().await.unwrap()["method"] == method {
                    break;
                }
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn shared_stream_waits_for_unsubscribe_before_eof() {
        let client = super::super::super::tests::fixture(
            r##"read -r thread
printf '%s\n' '{"id":1,"result":{"thread":{"id":"thread"}}}'
read -r turn
printf '%s\n' '{"id":2,"result":{"turn":{"id":"turn"}}}' '{"method":"item/agentMessage/delta","params":{"threadId":"thread","itemId":"final","delta":"# Summary"}}' '{"method":"turn/completed","params":{"threadId":"thread","turn":{"status":"completed"}}}'
read -r unsubscribe
case "$unsubscribe" in *thread/unsubscribe*) ;; *) exit 1 ;; esac
printf '%s\n' '{"method":"loofah/test/waiting"}'
read -r release
printf '%s\n' '{"id":3,"result":{}}' '{"id":4,"result":{}}'
read -r hold"##,
        );
        let mut events = client.notifications.subscribe();
        let mut stream = generation_stream(
            client.clone(),
            request(),
            CancellationToken::new(),
            CancellationToken::new(),
        );
        assert!(
            matches!(stream.next().await.unwrap().unwrap(), ModelChunk::Text(text) if text == "# Summary")
        );
        assert!(matches!(
            stream.next().await.unwrap().unwrap(),
            ModelChunk::Finish { truncated: false }
        ));
        notification(&mut events, "loofah/test/waiting").await;
        let mut eof = std::pin::pin!(stream.next());
        assert!(poll!(eof.as_mut()).is_pending());
        client.request("test/release", json!({})).await.unwrap();
        assert!(eof.await.is_none());
        assert!(client.alive.load(Ordering::SeqCst));
        client.stop();
    }

    #[tokio::test]
    async fn cancellation_drains_runtime_before_reporting_interruption() {
        for account_shutdown in [false, true] {
            let client = super::super::super::tests::fixture(
                r##"read -r thread
printf '%s\n' '{"id":1,"result":{"thread":{"id":"thread"}}}'
read -r turn
printf '%s\n' '{"id":2,"result":{"turn":{"id":"turn"}}}' '{"method":"item/agentMessage/delta","params":{"threadId":"thread","itemId":"final","delta":"# Started"}}'
read -r interrupt
case "$interrupt" in *turn/interrupt*) ;; *) exit 1 ;; esac
printf '%s\n' '{"id":3,"result":{}}'
read -r unsubscribe
printf '%s\n' '{"method":"loofah/test/waiting"}'
read -r release
printf '%s\n' '{"id":4,"result":{}}' '{"id":5,"result":{}}'
read -r hold"##,
            );
            let mut events = client.notifications.subscribe();
            let cancel = CancellationToken::new();
            let account_cancel = CancellationToken::new();
            let mut stream = generation_stream(
                client.clone(),
                request(),
                cancel.clone(),
                account_cancel.clone(),
            );
            assert!(matches!(
                stream.next().await.unwrap().unwrap(),
                ModelChunk::Text(_)
            ));
            if account_shutdown {
                account_cancel.cancel();
            } else {
                cancel.cancel();
            }
            notification(&mut events, "loofah/test/waiting").await;
            let mut next = std::pin::pin!(stream.next());
            assert!(poll!(next.as_mut()).is_pending());
            client.request("test/release", json!({})).await.unwrap();
            assert!(matches!(
                next.await,
                Some(Err(ProcessingJobFailure::Interrupted))
            ));
            assert!(stream.next().await.is_none());
            client.stop();
        }
    }

    #[tokio::test]
    async fn dropping_an_unconsumed_stream_still_interrupts_and_unsubscribes() {
        let client = super::super::super::tests::fixture(
            r##"read -r thread
printf '%s\n' '{"id":1,"result":{"thread":{"id":"thread"}}}'
read -r turn
printf '%s\n' '{"id":2,"result":{"turn":{"id":"turn"}}}' '{"method":"item/agentMessage/delta","params":{"threadId":"thread","itemId":"final","delta":"# Started"}}'
read -r interrupt
case "$interrupt" in *turn/interrupt*) ;; *) exit 1 ;; esac
printf '%s\n' '{"id":3,"result":{}}'
read -r unsubscribe
case "$unsubscribe" in *thread/unsubscribe*) ;; *) exit 1 ;; esac
printf '%s\n' '{"id":4,"result":{}}' '{"method":"loofah/test/drained"}'
read -r hold"##,
        );
        let mut events = client.notifications.subscribe();
        let mut stream = generation_stream(
            client.clone(),
            request(),
            CancellationToken::new(),
            CancellationToken::new(),
        );
        assert!(matches!(
            stream.next().await.unwrap().unwrap(),
            ModelChunk::Text(_)
        ));
        drop(stream);
        notification(&mut events, "loofah/test/drained").await;
        assert!(client.alive.load(Ordering::SeqCst));
        client.stop();
    }
}

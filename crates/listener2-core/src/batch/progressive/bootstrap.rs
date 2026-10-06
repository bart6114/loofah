use owhisper_client::{OpenAIAdapter, WhisperCppAdapter};
use ractor::{ActorProcessingErr, ActorRef};
use tracing::Instrument;

use super::ProgressiveProvider;
use super::actor::{
    BatchArgs, BatchMsg, BatchStartNotifier, process_provider_stream, report_stream_start_failure,
};

pub(super) async fn spawn_progressive_batch_task(
    args: BatchArgs,
    myself: ActorRef<BatchMsg>,
) -> Result<
    (
        tokio::task::JoinHandle<()>,
        tokio::sync::oneshot::Sender<()>,
    ),
    ActorProcessingErr,
> {
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let span = tracing::info_span!(
        "progressive_batch",
        fmtr.stt.provider.name = args.progressive_provider.label(),
        fmtr.session.id = %args.session_id,
        url.full = %args.base_url,
        fmtr.file.path = %args.file_path,
    );

    let rx_task = tokio::spawn(
        async move {
            let start = async {
                match args.progressive_provider {
                    ProgressiveProvider::WhisperCpp => {
                        WhisperCppAdapter::transcribe_file_streaming(
                            &args.base_url,
                            &args.listen_params,
                            &args.file_path,
                        )
                        .await
                    }
                    ProgressiveProvider::OpenAI => {
                        OpenAIAdapter::transcribe_file_streaming(
                            &args.base_url,
                            &args.api_key,
                            &args.listen_params,
                            &args.file_path,
                        )
                        .await
                    }
                }
            };
            let result = tokio::select! {
                _ = &mut shutdown_rx => return,
                result = start => result,
            };
            let stream = match result {
                Ok(stream) => {
                    notify_start_result(&args.start_notifier, Ok(()));
                    stream
                }
                Err(err) => {
                    report_stream_start_failure(
                        &myself,
                        &args.start_notifier,
                        &args.provider_label,
                        &err,
                        "progressive batch failed to start",
                    );
                    return;
                }
            };

            process_provider_stream(
                stream,
                myself,
                shutdown_rx,
                args.progressive_provider,
                "progressive batch",
            )
            .await;
        }
        .instrument(span),
    );

    Ok((rx_task, shutdown_tx))
}

pub(super) fn notify_start_result(notifier: &BatchStartNotifier, result: crate::Result<()>) {
    if let Ok(mut guard) = notifier.lock()
        && let Some(sender) = guard.take()
    {
        let _ = sender.send(result);
    }
}

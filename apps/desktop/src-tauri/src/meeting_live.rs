use std::sync::{Arc, Mutex};
use std::time::Instant;

use hypr_meeting_live::{self as live, LiveError, LiveState, RecordingState, Request, Response};
use tauri::{AppHandle, Listener, Manager};
use tauri_plugin_settings::SettingsPluginExt;
use tauri_plugin_transcription::{CaptureDataEvent, CaptureLifecycleEvent};
use tauri_specta::Event;

use crate::session_store::SessionStore;

pub struct MeetingLive {
    task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}
impl MeetingLive {
    pub fn shutdown(&self) {
        if let Some(task) = self.task.lock().unwrap().take() {
            task.abort();
        }
    }
}

pub fn spawn(app: AppHandle, store: Arc<SessionStore>) {
    let state = Arc::new(Mutex::new(LiveState::default()));
    let lifecycle_state = state.clone();
    let lifecycle_app = app.clone();
    CaptureLifecycleEvent::listen(&app, move |event| {
        let mut state = lifecycle_state.lock().unwrap();
        let finalizing = matches!(&event.payload, CaptureLifecycleEvent::Finalizing { .. });
        match event.payload {
            CaptureLifecycleEvent::Started {
                session_id,
                live_transcription_active,
                ..
            } => {
                let previous = state.active();
                let recording_id = state.start(&session_id, live_transcription_active);
                if let Some(previous) = previous.filter(|r| r.recording_id != recording_id) {
                    clear_nudge(&lifecycle_app, Some(previous.recording_id));
                }
            }
            CaptureLifecycleEvent::Finalizing { session_id }
            | CaptureLifecycleEvent::Stopped { session_id, .. } => {
                let previous = state.active().filter(|r| r.meeting_id == session_id);
                let phase = if finalizing {
                    RecordingState::Finalizing
                } else {
                    RecordingState::Stopped
                };
                state.transition(&session_id, phase);
                if let Some(previous) = previous {
                    clear_nudge(&lifecycle_app, Some(previous.recording_id));
                }
            }
        }
    });
    let data_state = state.clone();
    CaptureDataEvent::listen(&app, move |event| {
        if let CaptureDataEvent::TranscriptDelta { session_id, delta } = event.payload {
            let words = delta
                .new_words
                .into_iter()
                .map(|word| live::Word {
                    id: word.id,
                    text: word.text,
                    start_ms: word.start_ms,
                    end_ms: word.end_ms,
                    channel: word.channel,
                    speaker_index: word.speaker_index,
                    speaker_label: if word.channel == 0 {
                        "You".into()
                    } else {
                        word.speaker_index
                            .map(|i| format!("Speaker {}", i.saturating_add(1)))
                            .unwrap_or_else(|| "Speaker".into())
                    },
                })
                .collect();
            data_state
                .lock()
                .unwrap()
                .append(&session_id, words, delta.replaced_ids);
        }
    });
    let config_app = app.clone();
    app.listen(tauri_plugin_settings::CONFIG_CHANGED_EVENT, move |_| {
        if !config_app.settings().config().floating_bar_enabled {
            clear_nudge(&config_app, None);
        }
    });

    let server_state = state.clone();
    let server_app = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        // Vault relocation is applied by an app restart; each instance binds only its startup vault.
        if server_app
            .state::<crate::startup::StartupState>()
            .wait_until_ready()
            .await
            .is_err()
        {
            return;
        }
        let channel = match server_app.config().identifier.as_str() {
            id if id.ends_with(".dev") => "dev",
            id if id.ends_with(".staging") => "staging",
            _ => "stable",
        };
        let identity = match live::Identity::new(channel, store.vault_base()) {
            Ok(identity) => identity,
            Err(error) => {
                tracing::warn!(code = %error.code, "live endpoint unavailable");
                return;
            }
        };
        let server = match live::transport::Server::bind(&identity) {
            Ok(server) => server,
            Err(error) => {
                tracing::warn!(code = %error.code, "live endpoint unavailable");
                return;
            }
        };
        let capacity = Arc::new(tokio::sync::Semaphore::new(8));
        loop {
            let Ok((mut stream, _)) = server.listener.accept().await else {
                break;
            };
            let Ok(permit) = capacity.clone().try_acquire_owned() else {
                continue;
            };
            let identity = identity.clone();
            let state = server_state.clone();
            let app = server_app.clone();
            let store = store.clone();
            tauri::async_runtime::spawn(async move {
                let _permit = permit;
                let deadline = Instant::now() + live::REQUEST_TIMEOUT;
                let operation = async {
                    live::transport::verify_peer(&stream)?;
                    let envelope: live::Envelope = match live::transport::read(&mut stream).await {
                        Ok(envelope) => envelope,
                        Err(error) => {
                            return live::transport::write(&mut stream, &Response::Error(error))
                                .await;
                        }
                    };
                    let response = if let Err(error) = envelope.validate(&identity) {
                        Response::Error(error)
                    } else {
                        match handle(&app, &store, &state, envelope.request, deadline).await {
                            Ok(response) => response,
                            Err(error) => Response::Error(error),
                        }
                    };
                    match live::transport::write(&mut stream, &response).await {
                        Err(error) if error.code == "response_too_large" => {
                            live::transport::write(&mut stream, &Response::Error(error)).await
                        }
                        result => result,
                    }
                };
                let _ = tokio::time::timeout(live::REQUEST_TIMEOUT, operation).await;
            });
        }
    });
    app.manage(MeetingLive {
        task: Mutex::new(Some(task)),
    });
}

fn clear_nudge(app: &AppHandle, recording_id: Option<String>) {
    let _ = app.run_on_main_thread(move || {
        tauri_plugin_windows::clear_meeting_nudge(recording_id.as_deref());
    });
}

async fn handle(
    app: &AppHandle,
    store: &SessionStore,
    state: &Arc<Mutex<LiveState>>,
    request: Request,
    deadline: Instant,
) -> live::Result<Response> {
    match request {
        Request::Status => {
            let mut active = state.lock().unwrap().active();
            if let Some(meeting) = &mut active {
                meeting.title = store
                    .session_get(&meeting.meeting_id)
                    .map(|r| r.meta.title)
                    .unwrap_or_default();
                meeting.nudges_available = app.settings().config().floating_bar_enabled;
            }
            Ok(Response::Status(live::Status {
                active_meeting: active,
            }))
        }
        Request::Transcript(input) => Ok(Response::Transcript(
            state.lock().unwrap().transcript(&input)?,
        )),
        Request::Nudge(input) => {
            let text = live::normalize_nudge(&input.text)?;
            let recording = state
                .lock()
                .unwrap()
                .check_recording(input.recording_id.as_deref())?;
            let state = state.clone();
            let handle = app.clone();
            let (tx, rx) = tokio::sync::oneshot::channel();
            app.run_on_main_thread(move || {
                let result = (|| {
                    if tx.is_closed() || Instant::now() >= deadline {
                        return Err(LiveError::new("delivery_timeout", "Nudge request expired"));
                    }
                    let state = state.lock().unwrap();
                    state.check_recording(Some(&recording.recording_id))?;
                    if !handle.settings().config().floating_bar_enabled {
                        return Err(LiveError::new(
                            "bar_disabled",
                            "Enable Show floating bar in Settings → App → Meetings",
                        ));
                    }
                    let nudge_id =
                        tauri_plugin_windows::show_meeting_nudge(&recording.recording_id, &text)
                            .map_err(|_| {
                                LiveError::new(
                                    "bar_unavailable",
                                    "Recording bar is not ready; wait until it is visible",
                                )
                            })?;
                    Ok(Response::Nudge(live::NudgeReceipt {
                        nudge_id,
                        recording_id: recording.recording_id,
                    }))
                })();
                let _ = tx.send(result);
            })
            .map_err(|_| {
                LiveError::new("desktop_unavailable", "Desktop main thread is unavailable")
            })?;
            rx.await.map_err(|_| {
                LiveError::new(
                    "desktop_unavailable",
                    "Desktop closed before accepting nudge",
                )
            })?
        }
    }
}

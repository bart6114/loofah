use tauri::{AppHandle, Manager, Runtime};

use crate::{
    DetectEvent, ProcessorState,
    env::{Env, TauriEnv},
};

pub fn setup<R: Runtime>(app: &AppHandle<R>) -> Result<(), Box<dyn std::error::Error>> {
    let env = TauriEnv {
        app_handle: app.app_handle().clone(),
    };
    let processor = app.state::<ProcessorState>().inner().clone();

    let callback = hypr_detect::new_callback(move |event| {
        let env = env.clone();
        let processor = processor.clone();
        tauri::async_runtime::spawn(async move {
            handle_detect_event(&env, &processor, event);
        });
    });

    let detector_state = app.state::<crate::DetectorState>();
    let mut detector = detector_state.lock().unwrap_or_else(|e| e.into_inner());
    detector.start(callback);
    drop(detector);

    Ok(())
}

pub fn handle_detect_event<E: Env>(
    env: &E,
    state: &ProcessorState,
    event: hypr_detect::DetectEvent,
) {
    match event {
        hypr_detect::DetectEvent::MicStarted(apps) => {
            if !env.is_detect_enabled() {
                return;
            }
            handle_mic_started(env, state, apps);
        }
        hypr_detect::DetectEvent::MicStopped(apps) => {
            env.emit(DetectEvent::MicStopped { apps });
        }
        #[cfg(all(any(target_os = "macos", target_os = "windows"), feature = "sleep"))]
        hypr_detect::DetectEvent::SleepStateChanged { value } => {
            env.emit(DetectEvent::SleepStateChanged { value });
        }
    }
}

fn handle_mic_started<E: Env>(
    env: &E,
    state: &ProcessorState,
    apps: Vec<hypr_detect::InstalledApp>,
) {
    let events = {
        let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
        let mut events = Vec::new();
        for app in apps {
            if !guard.policy.should_track_app(&app.id) || !guard.mic_usage_tracker.claim(&app.id) {
                continue;
            }
            if guard.policy.respect_dnd && env.is_do_not_disturb() {
                continue;
            }
            tracing::info!(app_id = %app.id, "mic_detected");
            events.push(DetectEvent::MicDetected {
                key: uuid::Uuid::new_v4().to_string(),
                apps: vec![app],
                duration_secs: 0,
            });
        }
        events
    };

    for event in events {
        env.emit(event);
    }
}

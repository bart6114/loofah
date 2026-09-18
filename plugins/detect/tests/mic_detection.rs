use std::time::Duration;

use tauri_plugin_detect::env::test_support::TestEnv;
use tauri_plugin_detect::handler::handle_detect_event;
use tauri_plugin_detect::{DetectEvent, ProcessorState};

fn zoom() -> hypr_detect::InstalledApp {
    hypr_detect::InstalledApp {
        id: "us.zoom.xos".to_string(),
        name: "zoom.us".to_string(),
    }
}

fn aqua_voice() -> hypr_detect::InstalledApp {
    hypr_detect::InstalledApp {
        id: "com.electron.aqua-voice".to_string(),
        name: "Aqua Voice".to_string(),
    }
}

fn screen_studio() -> hypr_detect::InstalledApp {
    hypr_detect::InstalledApp {
        id: "com.timpler.screenstudio".to_string(),
        name: "Screen Studio".to_string(),
    }
}

fn snagit_2024() -> hypr_detect::InstalledApp {
    hypr_detect::InstalledApp {
        id: "com.TechSmith.Snagit2024".to_string(),
        name: "Snagit 2024".to_string(),
    }
}

fn slack() -> hypr_detect::InstalledApp {
    hypr_detect::InstalledApp {
        id: "com.tinyspeck.slackmacgap".to_string(),
        name: "Slack".to_string(),
    }
}

struct Harness {
    env: TestEnv,
    state: ProcessorState,
}

impl Harness {
    fn new() -> Self {
        Self {
            env: TestEnv::new(),
            state: ProcessorState::default(),
        }
    }

    fn mic_started(&self, app: hypr_detect::InstalledApp) {
        handle_detect_event(
            &self.env,
            &self.state,
            hypr_detect::DetectEvent::MicStarted(vec![app]),
        );
    }

    fn mic_stopped(&self, app: hypr_detect::InstalledApp) {
        handle_detect_event(
            &self.env,
            &self.state,
            hypr_detect::DetectEvent::MicStopped(vec![app]),
        );
    }

    fn take_events(&self) -> Vec<DetectEvent> {
        std::mem::take(&mut self.env.events.lock().unwrap())
    }
}

#[tokio::test(start_paused = true)]
async fn reminder_fires_immediately_without_advancing_time() {
    let h = Harness::new();
    h.mic_started(zoom());
    let events = h.take_events();
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], DetectEvent::MicDetected { apps, duration_secs: 0, .. } if apps.len() == 1 && apps[0].id == zoom().id)
    );
    tokio::time::advance(Duration::from_secs(30)).await;
    assert!(h.take_events().is_empty());
}

#[tokio::test(start_paused = true)]
async fn excluded_apps_do_not_prompt() {
    for app in [aqua_voice(), screen_studio(), snagit_2024()] {
        let h = Harness::new();
        h.mic_started(app);
        assert!(h.take_events().is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn user_ignored_app_does_not_prompt() {
    let h = Harness::new();
    h.state
        .lock()
        .unwrap()
        .policy
        .user_ignored_bundle_ids
        .insert(zoom().id);
    h.mic_started(zoom());
    assert!(h.take_events().is_empty());
    h.mic_started(slack());
    assert_eq!(h.take_events().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn user_included_app_prompts_unless_explicitly_ignored() {
    let h = Harness::new();
    h.state
        .lock()
        .unwrap()
        .policy
        .user_included_bundle_ids
        .insert(aqua_voice().id);
    h.mic_started(aqua_voice());
    assert_eq!(h.take_events().len(), 1);
    tokio::time::advance(Duration::from_secs(600)).await;
    h.state
        .lock()
        .unwrap()
        .policy
        .user_ignored_bundle_ids
        .insert(aqua_voice().id);
    h.mic_started(aqua_voice());
    assert!(h.take_events().is_empty());
}

#[tokio::test(start_paused = true)]
async fn dnd_suppresses_prompts_when_respected() {
    let h = Harness::new();
    h.env.set_dnd(true);
    h.state.lock().unwrap().policy.respect_dnd = true;
    h.mic_started(zoom());
    assert!(h.take_events().is_empty());
    h.env.set_dnd(false);
    h.mic_started(slack());
    assert_eq!(h.take_events().len(), 1);
    h.mic_started(zoom());
    assert!(
        h.take_events().is_empty(),
        "suppressed app retains its cooldown"
    );
}

#[tokio::test(start_paused = true)]
async fn dnd_does_not_suppress_prompts_when_not_respected() {
    let h = Harness::new();
    h.env.set_dnd(true);
    h.mic_started(zoom());
    assert_eq!(h.take_events().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn disabled_detection_does_not_prompt_or_start_cooldown() {
    let h = Harness::new();
    h.env.set_detect_enabled(false);
    h.mic_started(zoom());
    assert!(h.take_events().is_empty());
    h.env.set_detect_enabled(true);
    h.mic_started(zoom());
    assert_eq!(h.take_events().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn duplicate_events_and_rapid_restart_do_not_repeat_prompt() {
    let h = Harness::new();
    h.mic_started(zoom());
    assert_eq!(h.take_events().len(), 1);
    h.mic_started(zoom());
    assert!(h.take_events().is_empty());
    h.mic_stopped(zoom());
    assert!(matches!(
        &h.take_events()[0],
        DetectEvent::MicStopped { .. }
    ));
    h.mic_started(zoom());
    assert!(h.take_events().is_empty());
    tokio::time::advance(Duration::from_secs(599)).await;
    h.mic_started(zoom());
    assert!(h.take_events().is_empty());
    tokio::time::advance(Duration::from_secs(1)).await;
    h.mic_started(zoom());
    assert_eq!(h.take_events().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn cooldowns_are_independent_per_app() {
    let h = Harness::new();
    h.mic_started(zoom());
    assert_eq!(h.take_events().len(), 1);
    tokio::time::advance(Duration::from_secs(300)).await;
    h.mic_started(slack());
    assert_eq!(h.take_events().len(), 1);
    tokio::time::advance(Duration::from_secs(300)).await;
    h.mic_started(zoom());
    h.mic_started(slack());
    let events = h.take_events();
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], DetectEvent::MicDetected { apps, .. } if apps.len() == 1 && apps[0].id == zoom().id)
    );
}

#[tokio::test(start_paused = true)]
async fn simultaneous_apps_are_filtered_and_deduplicated() {
    let h = Harness::new();
    handle_detect_event(
        &h.env,
        &h.state,
        hypr_detect::DetectEvent::MicStarted(vec![zoom(), slack(), zoom(), aqua_voice()]),
    );
    let events = h.take_events();
    assert_eq!(events.len(), 2);
    let mut ids: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            DetectEvent::MicDetected { apps, .. } => Some(apps[0].id.as_str()),
            _ => None,
        })
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["com.tinyspeck.slackmacgap", "us.zoom.xos"]);
}

#[tokio::test(start_paused = true)]
async fn adding_exclusion_prevents_future_prompts_after_cooldown() {
    let h = Harness::new();
    h.mic_started(zoom());
    assert_eq!(h.take_events().len(), 1);
    h.state
        .lock()
        .unwrap()
        .policy
        .user_ignored_bundle_ids
        .insert(zoom().id);
    tokio::time::advance(Duration::from_secs(600)).await;
    h.mic_started(zoom());
    assert!(h.take_events().is_empty());
}

#[tokio::test(start_paused = true)]
async fn stop_events_always_reach_recording_controls() {
    let h = Harness::new();
    h.env.set_detect_enabled(false);
    h.env.set_dnd(true);
    h.state.lock().unwrap().policy.respect_dnd = true;
    let apps = vec![zoom(), slack(), aqua_voice()];
    handle_detect_event(
        &h.env,
        &h.state,
        hypr_detect::DetectEvent::MicStopped(apps.clone()),
    );
    let events = h.take_events();
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], DetectEvent::MicStopped { apps: stopped } if stopped.iter().map(|app| &app.id).eq(apps.iter().map(|app| &app.id)))
    );
}

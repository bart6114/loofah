use super::{MobileState, Settings, persist, providers, recording, summary};
use hypr_fs_format::{TranscriptWithData, TranscriptWord};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::atomic::Ordering};
use tauri::{Emitter, Manager};
use tauri_plugin_mobile_native::MobileNativeExt;

#[derive(Clone, Serialize, Deserialize)]
pub struct Job {
    pub session_id: String,
    pub kind: String,
    pub state: String,
    pub progress: f64,
    pub error: Option<String>,
    pub manual_pause: bool,
    id: String,
    source_hash: String,
    expected_output: Option<String>,
    audio_path: Option<String>,
    duration: f64,
    offset: f64,
    words: Vec<TranscriptWord>,
    summary_parts: Vec<String>,
    summary_inputs: Vec<String>,
    #[serde(default)]
    summary_instructions: String,
    #[serde(default)]
    summary_version: usize,
    #[serde(default)]
    expected_title: Option<String>,
    #[serde(default)]
    title_summary: Option<String>,
    settings: Settings,
}

impl Job {
    fn new(session_id: &str, kind: &str, settings: Settings) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.into(),
            kind: kind.into(),
            state: "queued".into(),
            progress: 0.0,
            error: None,
            manual_pause: false,
            source_hash: String::new(),
            expected_output: None,
            audio_path: None,
            duration: 0.0,
            offset: 0.0,
            words: vec![],
            summary_parts: vec![],
            summary_inputs: vec![],
            summary_instructions: String::new(),
            summary_version: 2,
            expected_title: None,
            title_summary: None,
            settings,
        }
    }

    fn runnable(&self, summary_ready: bool) -> bool {
        !self.manual_pause && self.state != "failed" && (self.kind == "transcribe" || summary_ready)
    }

    fn can_resume(&self, replacement: &Self) -> bool {
        (self.kind == "transcribe" || self.summary_version == replacement.summary_version)
            && self.source_hash == replacement.source_hash
            && self.expected_output == replacement.expected_output
            && self.audio_path == replacement.audio_path
            && self.settings == replacement.settings
            && self.expected_title == replacement.expected_title
            && self.title_summary == replacement.title_summary
    }
}

pub fn load(root: &Path) -> Result<Vec<Job>, String> {
    let mut queue: Vec<Job> = match std::fs::read(root.join("jobs.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string())?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(e) => return Err(e.to_string()),
    };
    for job in &mut queue {
        if job.state == "running" {
            job.state = "paused".into();
        }
    }
    Ok(queue)
}

pub fn prune_deleted(
    queue: &mut Vec<Job>,
    root: &Path,
    mut is_deleted: impl FnMut(&str) -> Result<bool, String>,
) -> Result<(), String> {
    let mut deleted = std::collections::HashSet::new();
    for job in queue.iter() {
        if is_deleted(&job.session_id)? {
            deleted.insert(job.session_id.clone());
        }
    }
    if deleted.is_empty() {
        return Ok(());
    }
    let retained: Vec<_> = queue
        .iter()
        .filter(|job| !deleted.contains(&job.session_id))
        .collect();
    persist::write_json(&root.join("jobs.json"), &retained)?;
    queue.retain(|job| !deleted.contains(&job.session_id));
    Ok(())
}

pub fn dismiss_failed(queue: &mut Vec<Job>, root: &Path) -> Result<(), String> {
    let retained: Vec<_> = queue
        .iter()
        .filter(|job| job.state != "failed")
        .cloned()
        .collect();
    if retained.len() == queue.len() {
        return Ok(());
    }
    persist::write_json(&root.join("jobs.json"), &retained)?;
    *queue = retained;
    Ok(())
}

pub async fn enqueue(app: &tauri::AppHandle, session_id: &str, kind: &str) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let directory = state.session_dir(session_id)?;
    let settings = state.settings.lock().unwrap().clone();
    let mut job = Job::new(session_id, kind, settings);
    if kind == "transcribe" {
        let path =
            hypr_vault_read::audio::resolve_final_audio_path(state.store.vault_base(), session_id)
                .ok_or("Download or record the audio first")?;
        let path_for_hash = path.clone();
        job.source_hash = tokio::task::spawn_blocking(move || persist::hash(&path_for_hash))
            .await
            .map_err(|e| e.to_string())??;
        job.expected_output = persist::optional_hash(&directory.join("transcript.json"))?;
        job.duration = app
            .mobile_native()
            .audio_duration(path.to_string_lossy().as_ref())
            .await
            .map_err(|e| e.to_string())?;
        if !job.duration.is_finite() || job.duration <= 0.0 {
            return Err("Recording contains no audio".into());
        }
        job.audio_path = Some(path.to_string_lossy().into_owned());
    } else if kind == "title" {
        let Some((prepared, expected_title, title_summary)) =
            summary::prepare_title(&state.store, session_id, &job.settings.summary_language)
                .await?
        else {
            let mut queue = state.jobs.lock().unwrap();
            let mut retained = queue.clone();
            retained.retain(|job| job.session_id != session_id || job.kind != "title");
            persist::write_json(&state.state_dir.join("jobs.json"), &retained)?;
            *queue = retained;
            drop(queue);
            state.changed(app);
            return Ok(());
        };
        providers::availability(app, &job.settings).await?;
        job.source_hash = prepared.source_hash;
        job.summary_inputs = vec![prepared.input];
        job.summary_instructions = prepared.instructions;
        job.expected_title = Some(expected_title);
        job.title_summary = Some(title_summary);
    } else {
        providers::availability(app, &job.settings).await?;
        let prepared =
            summary::prepare(&state.store, session_id, &job.settings.summary_language).await?;
        job.source_hash = prepared.source_hash;
        job.expected_output = persist::optional_hash(&directory.join("summary.md"))?;
        job.summary_inputs = vec![prepared.input];
        job.summary_instructions = prepared.instructions;
    }
    {
        let mut queue = state.jobs.lock().unwrap();
        if let Some(existing) = queue
            .iter_mut()
            .find(|j| j.session_id == session_id && j.kind == kind)
        {
            if existing.state == "running" || state.worker.load(Ordering::Acquire) {
                return Err("Wait for the current processing segment to pause".into());
            }
            if existing.can_resume(&job) {
                existing.state = "queued".into();
                existing.manual_pause = false;
                existing.error = None;
            } else {
                *existing = job;
            }
        } else {
            queue.push(job);
        }
        persist::write_json(&state.state_dir.join("jobs.json"), &*queue)?;
    }
    state.changed(app);
    Ok(())
}

pub fn record_failed_transcription(
    app: &tauri::AppHandle,
    session_id: &str,
    error: &str,
) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let mut failed = Job::new(
        session_id,
        "transcribe",
        state.settings.lock().unwrap().clone(),
    );
    failed.state = "failed".into();
    failed.error = Some(error.into());
    let mut queue = state.jobs.lock().unwrap();
    let mut retained = queue.clone();
    retained.retain(|job| job.session_id != session_id || job.kind != "transcribe");
    retained.push(failed);
    persist::write_json(&state.state_dir.join("jobs.json"), &retained)?;
    *queue = retained;
    state.changed(app);
    Ok(())
}

pub fn kick(app: &tauri::AppHandle) {
    let state = app.state::<MobileState>();
    let summary_ready = app
        .state::<providers::Bridge>()
        .ready
        .load(Ordering::Acquire);
    if recording::is_active()
        || recording::is_stopping()
        || !recording::FOREGROUND.load(Ordering::Acquire)
        || state.model_busy.load(Ordering::Acquire)
    {
        return;
    }
    if !state
        .jobs
        .lock()
        .unwrap()
        .iter()
        .any(|job| job.runnable(summary_ready))
    {
        return;
    }
    if state
        .worker
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        struct Worker(tauri::AppHandle);
        impl Drop for Worker {
            fn drop(&mut self) {
                self.0
                    .state::<MobileState>()
                    .worker
                    .store(false, Ordering::Release);
            }
        }
        let _worker = Worker(app.clone());
        let state = app.state::<MobileState>();
        let next = state
            .jobs
            .lock()
            .unwrap()
            .iter()
            .find(|job| job.runnable(summary_ready))
            .cloned();
        if let Some(mut job) = next {
            let result = process(&app, &mut job).await;
            if let Err(error) = result {
                let mut queue = state.jobs.lock().unwrap();
                if let Some(current) = queue.iter_mut().find(|j| j.id == job.id) {
                    current.state = "failed".into();
                    current.error = Some(error);
                    if let Err(error) =
                        persist::write_json(&state.state_dir.join("jobs.json"), &*queue)
                    {
                        let _ = app.emit("mobile-startup-error", error);
                    }
                }
            }
            state.changed(&app);
        }
    });
}

fn checkpoint(app: &tauri::AppHandle, job: &mut Job) -> Result<bool, String> {
    let state = app.state::<MobileState>();
    let mut queue = state.jobs.lock().unwrap();
    let current = queue
        .iter_mut()
        .find(|j| j.id == job.id)
        .ok_or("Job no longer exists")?;
    job.manual_pause = current.manual_pause;
    let paused = job.manual_pause
        || (job.kind != "transcribe"
            && !app
                .state::<providers::Bridge>()
                .ready
                .load(Ordering::Acquire))
        || state.model_busy.load(Ordering::Acquire)
        || recording::is_active()
        || !recording::FOREGROUND.load(Ordering::Acquire);
    job.state = if paused { "paused" } else { "running" }.into();
    *current = job.clone();
    persist::write_json(&state.state_dir.join("jobs.json"), &*queue)?;
    let _ = app.emit("mobile-state-changed", ());
    Ok(!paused)
}

async fn process(app: &tauri::AppHandle, job: &mut Job) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let mut followup = None;
    let initial_operation = state.operation.lock().await;
    if !checkpoint(app, job)? {
        return Ok(());
    }
    let directory = state.session_dir(&job.session_id)?;
    drop(initial_operation);
    if job.kind == "transcribe" {
        if !app
            .mobile_native()
            .model_status(&job.settings.transcription_model)
            .await
            .map_err(|e| e.to_string())?
            .ready
        {
            return Err("Download the transcription model, then retry this recording".into());
        }
        let path = job
            .audio_path
            .as_ref()
            .ok_or("Job has no recording")?
            .clone();
        let path_copy = path.clone();
        let current_hash =
            tokio::task::spawn_blocking(move || persist::hash(Path::new(&path_copy)))
                .await
                .map_err(|e| e.to_string())??;
        if current_hash != job.source_hash {
            return Err("Recording changed; preserved the existing transcript".into());
        }
        let capture_meta = state
            .store
            .read_meta(&job.session_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or("Session was deleted")?;
        let audio = native_session_audio(&capture_meta);
        while job.offset < job.duration {
            if !checkpoint(app, job)? {
                return Ok(());
            }
            let window_start = (job.offset - 1.0).max(0.0);
            let commit_end = (job.offset + 28.0).min(job.duration);
            let window_end = (commit_end + 1.0).min(job.duration);
            let words = app
                .mobile_native()
                .transcribe_window(
                    &job.settings.transcription_model,
                    &path,
                    window_start,
                    window_end - window_start,
                )
                .await
                .map_err(|e| e.to_string())?;
            job.words.extend(native_window_words(
                words,
                window_start,
                job.offset,
                commit_end,
                job.duration,
                audio,
            ));
            job.offset = commit_end;
            job.progress = job.offset / job.duration;
            if !checkpoint(app, job)? {
                return Ok(());
            }
        }
        if job.words.is_empty() {
            return Err("The speech model returned no words; existing transcript preserved".into());
        }
        let _operation = state.operation.lock().await;
        if !checkpoint(app, job)? {
            return Ok(());
        }
        state.session_dir(&job.session_id)?;
        let current_path = path.clone();
        if tokio::task::spawn_blocking(move || persist::hash(Path::new(&current_path)))
            .await
            .map_err(|e| e.to_string())??
            != job.source_hash
        {
            return Err(
                "Recording changed while transcribing; existing transcript preserved".into(),
            );
        }
        if persist::optional_hash(&directory.join("transcript.json"))? != job.expected_output {
            return Err("Transcript changed while processing; existing text was preserved".into());
        }
        let meta = state
            .store
            .read_meta(&job.session_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Session was deleted")?;
        let started = meta
            .started_at
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.timestamp_millis() as f64)
            .unwrap_or(0.0);
        let existing = state
            .store
            .session_transcripts(&job.session_id)
            .await
            .map_err(|e| e.to_string())?;
        if existing.len() > 1 {
            return Err("This session has multiple transcript segments; re-transcribe it on the Mac to preserve its structure".into());
        }
        let id = existing
            .first()
            .map(|t| t.id.clone())
            .unwrap_or_else(|| job.id.clone());
        state
            .store
            .write_transcript(
                &job.session_id,
                TranscriptWithData {
                    id,
                    user_id: existing
                        .first()
                        .map(|t| t.user_id.clone())
                        .unwrap_or_default(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                    session_id: job.session_id.clone(),
                    started_at: started,
                    ended_at: Some(started + job.duration * 1000.0),
                    memo_md: existing
                        .first()
                        .map(|t| t.memo_md.clone())
                        .unwrap_or_default(),
                    words: job.words.clone(),
                    speaker_hints: vec![],
                },
            )
            .await
            .map_err(|e| e.to_string())?;
    } else if job.kind == "title" {
        if job.summary_version != 2
            || job.summary_inputs.len() != 1
            || job.summary_instructions.is_empty()
            || job.expected_title.is_none()
            || job.title_summary.is_none()
        {
            return Err("Title setup changed. Retry title generation.".into());
        }
        let meta = state
            .store
            .read_meta(&job.session_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Session was deleted")?;
        // A manual title wins, including an edit made while this job was paused.
        if meta.title == *job.expected_title.as_ref().unwrap() {
            if state
                .store
                .read_summary(&job.session_id)
                .await
                .map_err(|e| e.to_string())?
                .as_deref()
                != job.title_summary.as_deref()
            {
                return Err(
                    "Summary changed. Retry title generation using the saved summary.".into(),
                );
            }
            if job.summary_parts.is_empty() {
                match providers::generate(
                    app,
                    &job.session_id,
                    &job.settings,
                    &job.summary_instructions,
                    &job.summary_inputs[0],
                    Some(128),
                )
                .await?
                {
                    Some(output) => job.summary_parts.push(output),
                    None => {
                        checkpoint(app, job)?;
                        return Ok(());
                    }
                }
                job.progress = 0.95;
                if !checkpoint(app, job)? {
                    return Ok(());
                }
            }
            let _operation = state.operation.lock().await;
            if !checkpoint(app, job)? {
                return Ok(());
            }
            if Settings::load(state.store.vault_base())? != job.settings {
                return Err("Title settings changed. Retry with the selected provider.".into());
            }
            state
                .store
                .apply_generated_title(
                    &job.session_id,
                    &job.summary_parts[0],
                    job.expected_title.as_deref().unwrap(),
                    job.title_summary.as_deref().unwrap(),
                )
                .await
                .map_err(|e| e.to_string())?;
        }
    } else {
        if job.summary_version != 2
            || job.summary_instructions.is_empty()
            || job.summary_inputs.len() != 1
        {
            return Err("Summary setup changed. Retry with a configured summary provider.".into());
        }
        if job.summary_parts.is_empty() {
            if !checkpoint(app, job)? {
                return Ok(());
            }
            match providers::generate(
                app,
                &job.session_id,
                &job.settings,
                &job.summary_instructions,
                &job.summary_inputs[0],
                None,
            )
            .await?
            {
                Some(output) => job.summary_parts.push(output),
                None => {
                    checkpoint(app, job)?;
                    return Ok(());
                }
            }
            job.progress = 0.95;
            if !checkpoint(app, job)? {
                return Ok(());
            }
        }
        let _operation = state.operation.lock().await;
        if !checkpoint(app, job)? {
            return Ok(());
        }
        let output = job.summary_parts.first().ok_or("No summary generated")?;
        state.session_dir(&job.session_id)?;
        let expected = state
            .store
            .read_summary(&job.session_id)
            .await
            .map_err(|e| e.to_string())?;
        if Settings::load(state.store.vault_base())? != job.settings
            || summary::prepare(
                &state.store,
                &job.session_id,
                &job.settings.summary_language,
            )
            .await?
            .source_hash
                != job.source_hash
            || (persist::optional_hash(&directory.join("summary.md"))? != job.expected_output
                && expected.as_deref() != Some(output.as_str()))
        {
            return Err(
                "Notes, transcript, summary or prompt changed; generated output was not applied"
                    .into(),
            );
        }
        save_generated_summary(&state.store, &job.session_id, output, expected.as_deref()).await?;
        if let Some((prepared, expected_title, title_summary)) = summary::prepare_title(
            &state.store,
            &job.session_id,
            &job.settings.summary_language,
        )
        .await?
        {
            let mut title = job.clone();
            title.id = uuid::Uuid::new_v4().to_string();
            title.kind = "title".into();
            title.state = "queued".into();
            title.progress = 0.0;
            title.error = None;
            title.source_hash = prepared.source_hash;
            title.expected_output = None;
            title.summary_parts.clear();
            title.summary_inputs = vec![prepared.input];
            title.summary_instructions = prepared.instructions;
            title.expected_title = Some(expected_title);
            title.title_summary = Some(title_summary);
            followup = Some(title);
        }
    }
    if job.kind == "transcribe" {
        let selected = state.settings.lock().unwrap().clone();
        if !matches!(selected.summary_provider.as_str(), "none" | "") {
            followup = Some(prepare_followup_summary(app, job, selected).await);
        }
    }
    {
        let mut queue = state.jobs.lock().unwrap();
        let replace_kind = (job.kind == "transcribe").then_some("summary");
        complete_with_followup(
            &mut queue,
            &state.state_dir,
            &job.id,
            replace_kind,
            followup,
        )?;
    }
    state.changed(app);
    Ok(())
}

async fn prepare_followup_summary(
    app: &tauri::AppHandle,
    completed: &Job,
    selected: Settings,
) -> Job {
    let state = app.state::<MobileState>();
    let mut followup = Job::new(&completed.session_id, "summary", selected);
    let result = async {
        providers::availability(app, &followup.settings).await?;
        let prepared = summary::prepare(
            &state.store,
            &followup.session_id,
            &followup.settings.summary_language,
        )
        .await?;
        followup.source_hash = prepared.source_hash;
        followup.expected_output =
            persist::optional_hash(&state.session_dir(&followup.session_id)?.join("summary.md"))?;
        followup.summary_inputs = vec![prepared.input];
        followup.summary_instructions = prepared.instructions;
        Ok::<(), String>(())
    }
    .await;
    if let Err(error) = result {
        followup.state = "failed".into();
        followup.error = Some(error);
    }
    followup
}

fn complete_with_followup(
    queue: &mut Vec<Job>,
    root: &Path,
    completed_id: &str,
    replace_kind: Option<&str>,
    mut followup: Option<Job>,
) -> Result<(), String> {
    let completed = queue
        .iter()
        .find(|job| job.id == completed_id)
        .ok_or("Job no longer exists")?;
    if completed.manual_pause {
        if let Some(next) = &mut followup {
            next.manual_pause = true;
            if next.state != "failed" {
                next.state = "paused".into();
            }
        }
    }
    let mut retained = queue.clone();
    retained.retain(|job| job.id != completed_id);
    if let Some(kind) = replace_kind {
        retained.retain(|job| job.session_id != completed.session_id || job.kind != kind);
    }
    if let Some(followup) = followup {
        retained.retain(|job| job.session_id != followup.session_id || job.kind != followup.kind);
        retained.push(followup);
    }
    persist::write_json(&root.join("jobs.json"), &retained)?;
    *queue = retained;
    Ok(())
}

fn native_session_audio(meta: &hypr_vault_read::SessionMeta) -> hypr_fs_format::SessionAudio {
    let mut audio = meta
        .extra
        .get("audio")
        .cloned()
        .and_then(|value| serde_json::from_value::<hypr_fs_format::SessionAudio>(value).ok())
        .unwrap_or_default();
    // Native windows are mono even when the original file had separate channels.
    audio.layout = hypr_fs_format::AudioLayout::Mixed;
    audio
}

fn native_window_words(
    words: Vec<tauri_plugin_mobile_native::NativeWord>,
    window_start: f64,
    offset: f64,
    commit_end: f64,
    duration: f64,
    audio: hypr_fs_format::SessionAudio,
) -> Vec<TranscriptWord> {
    use owhisper_interface::batch;
    let words: Vec<_> = words
        .into_iter()
        .filter_map(|word| {
            let (start, end) = committed_word_bounds(
                word.start,
                word.end,
                window_start,
                offset,
                commit_end,
                duration,
            )?;
            let text = word.text.trim().to_string();
            if text.is_empty() {
                return None;
            }
            Some(batch::Word {
                word: text.clone(),
                punctuated_word: Some(text),
                start,
                end,
                confidence: 1.0,
                channel: 2,
                speaker: None,
            })
        })
        .collect();
    let transcript = words
        .iter()
        .map(|word| word.word.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let response = batch::Response {
        metadata: serde_json::json!({"session_audio": audio, "duration": duration}),
        results: batch::Results {
            channels: vec![batch::Channel {
                alternatives: vec![batch::Alternatives {
                    transcript,
                    confidence: 1.0,
                    words,
                }],
            }],
        },
    };
    let (mut mapped, _) = hypr_transcript::batch::words_and_hints_from_batch_response(&response);
    for word in &mut mapped {
        word.id = Some(uuid::Uuid::new_v4().to_string());
    }
    mapped
}

fn committed_word_bounds(
    relative_start: f64,
    relative_end: f64,
    window_start: f64,
    offset: f64,
    commit_end: f64,
    duration: f64,
) -> Option<(f64, f64)> {
    let start = window_start + relative_start;
    let raw_end = window_start + relative_end;
    if !start.is_finite() || !raw_end.is_finite() || start < 0.0 || raw_end < start {
        return None;
    }
    let end = raw_end.min(duration);
    if end < start {
        return None;
    }
    // Overlap supplies acoustic context, but each midpoint belongs to one committed window.
    let midpoint = (start + end) / 2.0;
    if midpoint < offset || (commit_end < duration && midpoint >= commit_end) {
        return None;
    }
    Some((start, end))
}

async fn save_generated_summary(
    store: &hypr_vault_write::SessionStore,
    session_id: &str,
    output: &str,
    expected: Option<&str>,
) -> Result<(), String> {
    match expected {
        Some(expected) => {
            store
                .update_generated_summary(session_id, output, Some(expected))
                .await
        }
        None => store.create_generated_summary(session_id, output).await,
    }
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn first_generated_summary_is_saved_without_replacing_concurrent_edits() {
        let vault = tempfile::tempdir().unwrap();
        let store = hypr_vault_write::SessionStore::new(vault.path().to_owned());
        let meta = serde_json::from_value(serde_json::json!({
            "id": "session", "title": "Meeting", "created_at": "2026-09-18T12:00:00Z", "tags": []
        }))
        .unwrap();
        store.write_meta(&meta).await.unwrap();
        save_generated_summary(&store, "session", "First summary", None)
            .await
            .unwrap();
        assert_eq!(
            store.read_summary("session").await.unwrap().as_deref(),
            Some("First summary")
        );
        assert!(
            save_generated_summary(&store, "session", "Stale first result", None)
                .await
                .is_err()
        );
        store
            .update_summary("session", "User edit", Some("First summary"))
            .await
            .unwrap();
        assert!(
            save_generated_summary(
                &store,
                "session",
                "Stale regeneration",
                Some("First summary")
            )
            .await
            .is_err()
        );
        assert_eq!(
            store.read_summary("session").await.unwrap().as_deref(),
            Some("User edit")
        );
        store.delete_summary("session").await.unwrap();
        assert!(
            save_generated_summary(&store, "session", "Deleted regeneration", Some("User edit"))
                .await
                .is_err()
        );
        assert_eq!(store.read_summary("session").await.unwrap(), None);
    }

    fn transcription_job() -> Job {
        Job {
            session_id: "session".into(),
            kind: "transcribe".into(),
            state: "running".into(),
            progress: 28.0 / 60.0,
            error: None,
            manual_pause: false,
            id: "job".into(),
            source_hash: "audio-hash".into(),
            expected_output: Some("transcript-hash".into()),
            audio_path: Some("audio.wav".into()),
            duration: 60.0,
            offset: 28.0,
            words: vec![TranscriptWord {
                id: Some("word".into()),
                text: "first".into(),
                start_ms: 0.0,
                end_ms: 500.0,
                channel: 0.0,
                speaker: None,
                metadata: None,
            }],
            summary_parts: vec!["completed part".into()],
            summary_inputs: vec!["input".into()],
            summary_instructions: "Summarize the supplied material.".into(),
            summary_version: 2,
            expected_title: None,
            title_summary: None,
            settings: Settings::default(),
        }
    }

    #[test]
    fn completed_transcription_hands_off_to_persisted_summary_without_empty_queue() {
        let root = tempfile::tempdir().unwrap();
        let mut completed = transcription_job();
        completed.manual_pause = true;
        let mut stale = Job::new("session", "summary", Settings::default());
        stale.state = "failed".into();
        let mut queue = vec![completed.clone(), stale];
        persist::write_json(&root.path().join("jobs.json"), &queue).unwrap();

        let mut followup = Job::new("session", "summary", Settings::default());
        followup.error = Some("Choose a summary model".into());
        followup.state = "failed".into();
        complete_with_followup(
            &mut queue,
            root.path(),
            &completed.id,
            Some("summary"),
            Some(followup),
        )
        .unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].kind, "summary");
        assert_eq!(queue[0].state, "failed");
        assert!(queue[0].manual_pause);
        assert_eq!(queue[0].error.as_deref(), Some("Choose a summary model"));
        let restored = load(root.path()).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].kind, "summary");

        let before = std::fs::read(root.path().join("jobs.json")).unwrap();
        assert!(
            complete_with_followup(
                &mut queue,
                root.path(),
                &completed.id,
                Some("summary"),
                None,
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read(root.path().join("jobs.json")).unwrap(),
            before
        );
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn completed_transcription_without_provider_clears_stale_summary() {
        let root = tempfile::tempdir().unwrap();
        let completed = transcription_job();
        let stale = Job::new("session", "summary", Settings::default());
        let mut queue = vec![completed.clone(), stale];
        complete_with_followup(
            &mut queue,
            root.path(),
            &completed.id,
            Some("summary"),
            None,
        )
        .unwrap();
        assert!(queue.is_empty());
        assert!(load(root.path()).unwrap().is_empty());
    }

    #[test]
    fn followup_handoff_keeps_memory_if_persistence_fails() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("jobs.json")).unwrap();
        let completed = transcription_job();
        let mut queue = vec![completed.clone()];
        let followup = Job::new("session", "summary", Settings::default());
        assert!(
            complete_with_followup(
                &mut queue,
                root.path(),
                &completed.id,
                Some("summary"),
                Some(followup),
            )
            .is_err()
        );
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].id, completed.id);
    }

    #[test]
    fn title_jobs_preserve_generation_on_resume_and_require_the_bridge() {
        let mut job = transcription_job();
        job.kind = "title".into();
        job.expected_title = Some("".into());
        job.title_summary = Some("Saved summary".into());
        assert!(!job.runnable(false));
        assert!(job.runnable(true));
        let roundtrip: Job = serde_json::from_value(serde_json::to_value(&job).unwrap()).unwrap();
        assert!(job.can_resume(&roundtrip));
        assert_eq!(roundtrip.summary_parts, ["completed part"]);
        let mut edited = roundtrip;
        edited.title_summary = Some("Changed summary".into());
        assert!(!job.can_resume(&edited));
    }

    #[test]
    fn restart_preserves_checkpoint_and_manual_pause() {
        let root = tempfile::tempdir().unwrap();
        let running = transcription_job();
        let mut paused = running.clone();
        paused.id = "paused".into();
        paused.state = "paused".into();
        paused.manual_pause = true;
        persist::write_json(&root.path().join("jobs.json"), &vec![running, paused]).unwrap();
        let jobs = load(root.path()).unwrap();
        assert_eq!(jobs[0].state, "paused");
        assert!(!jobs[0].manual_pause);
        assert_eq!(jobs[0].offset, 28.0);
        assert_eq!(jobs[0].words[0].text, "first");
        assert_eq!(jobs[0].summary_parts, ["completed part"]);
        assert_eq!(jobs[0].summary_version, 2);
        assert!(jobs[1].manual_pause);
    }

    #[test]
    fn pruning_removes_only_explicitly_deleted_session_checkpoints() {
        let root = tempfile::tempdir().unwrap();
        let mut queue = vec![];
        for (id, state) in [
            ("deleted", "paused"),
            ("existing", "failed"),
            ("missing", "paused"),
        ] {
            let mut job = transcription_job();
            job.id = id.into();
            job.session_id = id.into();
            job.state = state.into();
            job.manual_pause = state == "paused";
            queue.push(job);
        }
        persist::write_json(&root.path().join("jobs.json"), &queue).unwrap();
        prune_deleted(&mut queue, root.path(), |id| Ok(id == "deleted")).unwrap();
        assert_eq!(
            queue
                .iter()
                .map(|j| j.session_id.as_str())
                .collect::<Vec<_>>(),
            ["existing", "missing"]
        );
        let restored = load(root.path()).unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].state, "failed");
        assert!(restored[1].manual_pause);
        assert_eq!(restored[1].words[0].text, "first");
        assert_eq!(restored[1].summary_inputs, ["input"]);
    }

    #[test]
    fn pruning_preserves_memory_when_persistence_or_deletion_check_fails() {
        let root = tempfile::tempdir().unwrap();
        let mut queue = vec![transcription_job()];
        persist::write_json(&root.path().join("jobs.json"), &queue).unwrap();
        let before = std::fs::read(root.path().join("jobs.json")).unwrap();
        assert!(
            prune_deleted(
                &mut queue,
                root.path(),
                |_| Err("unavailable marker".into())
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read(root.path().join("jobs.json")).unwrap(),
            before
        );
        std::fs::remove_file(root.path().join("jobs.json")).unwrap();
        std::fs::create_dir(root.path().join("jobs.json")).unwrap();
        assert!(prune_deleted(&mut queue, root.path(), |_| Ok(true)).is_err());
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].words[0].text, "first");
    }

    #[test]
    fn dismissing_failures_preserves_other_jobs_and_persists() {
        let root = tempfile::tempdir().unwrap();
        let mut failed = transcription_job();
        failed.state = "failed".into();
        let mut paused = transcription_job();
        paused.id = "paused".into();
        paused.state = "paused".into();
        let mut queue = vec![failed, paused];
        dismiss_failed(&mut queue, root.path()).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].id, "paused");
        assert_eq!(load(root.path()).unwrap().len(), 1);
    }

    #[test]
    fn resume_rejects_changed_source_output_path_or_settings() {
        let checkpoint = transcription_job();
        assert!(checkpoint.can_resume(&checkpoint.clone()));
        let mut replacement = checkpoint.clone();
        replacement.source_hash = "new-audio".into();
        assert!(!checkpoint.can_resume(&replacement));
        replacement = checkpoint.clone();
        replacement.expected_output = None;
        assert!(!checkpoint.can_resume(&replacement));
        replacement = checkpoint.clone();
        replacement.audio_path = Some("audio.mp3".into());
        assert!(!checkpoint.can_resume(&replacement));
        replacement = checkpoint.clone();
        replacement.settings.summary_language = "nl".into();
        assert!(!checkpoint.can_resume(&replacement));
        replacement = checkpoint.clone();
        replacement.settings.transcription_model = "parakeet-v2".into();
        assert!(!checkpoint.can_resume(&replacement));
    }

    #[test]
    fn overlap_boundary_belongs_to_next_window_once() {
        assert!(committed_word_bounds(27.8, 28.2, 0.0, 0.0, 28.0, 60.0).is_none());
        assert_eq!(
            committed_word_bounds(0.8, 1.2, 27.0, 28.0, 56.0, 60.0),
            Some((27.8, 28.2))
        );
        assert!(committed_word_bounds(0.0, 0.5, 27.0, 28.0, 56.0, 60.0).is_none());
        assert_eq!(
            committed_word_bounds(4.0, 5.0, 55.0, 56.0, 60.0, 60.0),
            Some((59.0, 60.0))
        );
    }

    #[test]
    fn malformed_word_timestamps_are_not_clamped_into_valid_words() {
        for end in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            assert!(committed_word_bounds(0.0, end, 0.0, 0.0, 28.0, 60.0).is_none());
        }
        assert!(committed_word_bounds(f64::NAN, 1.0, 0.0, 0.0, 28.0, 60.0).is_none());
        assert!(committed_word_bounds(61.0, 62.0, 0.0, 56.0, 60.0, 60.0).is_none());
    }

    #[test]
    fn summary_bridge_does_not_block_native_transcription() {
        let transcription = transcription_job();
        let mut summary = transcription.clone();
        summary.kind = "summary".into();
        let queue = [summary.clone(), transcription];
        assert_eq!(
            queue.iter().find(|job| job.runnable(false)).unwrap().kind,
            "transcribe"
        );
        assert_eq!(
            queue.iter().find(|job| job.runnable(true)).unwrap().kind,
            "summary"
        );
        summary.manual_pause = true;
        assert!(!summary.runnable(true));
        summary.manual_pause = false;
        summary.state = "failed".into();
        assert!(!summary.runnable(true));
    }

    #[test]
    fn legacy_transcription_version_preserves_partial_progress_on_resume() {
        let mut legacy = transcription_job();
        legacy.summary_version = 0;
        legacy.offset = 28.0;
        let current = transcription_job();
        assert!(legacy.can_resume(&current));
        assert_eq!(legacy.offset, 28.0);
        assert_eq!(legacy.words[0].text, "first");
    }

    #[test]
    fn old_summary_pipeline_cannot_resume_as_a_provider_summary() {
        let mut current = transcription_job();
        current.kind = "summary".into();
        let mut legacy = current.clone();
        legacy.summary_version = 0;
        assert!(!legacy.can_resume(&current));
    }
}

#[cfg(test)]
mod native_transcription_tests {
    use super::*;
    use hypr_fs_format::{AudioLayout, AudioSource, SessionAudio};
    use tauri_plugin_mobile_native::NativeWord;

    fn native(text: &str, start: f64, end: f64) -> NativeWord {
        NativeWord {
            text: text.into(),
            start,
            end,
        }
    }

    #[test]
    fn windows_keep_one_copy_of_seam_words_and_shared_spacing_metadata() {
        let audio = SessionAudio {
            source: AudioSource::Import,
            layout: AudioLayout::Mixed,
        };
        let mut first = native_window_words(
            vec![native(" First", 0.0, 0.4), native("seam", 27.8, 28.2)],
            0.0,
            0.0,
            28.0,
            60.0,
            audio,
        );
        let next = native_window_words(
            vec![
                native("context", 0.0, 0.5),
                native(" seam", 0.8, 1.2),
                native("next.", 1.3, 1.5),
            ],
            27.0,
            28.0,
            56.0,
            60.0,
            audio,
        );
        first.extend(next);
        assert_eq!(
            first
                .iter()
                .map(|word| word.text.as_str())
                .collect::<String>(),
            " First seam next."
        );
        assert_eq!((first[1].start_ms, first[1].end_ms), (27_800.0, 28_200.0));
        assert!(first.iter().all(|word| word.channel == 2.0));
        assert!(
            first
                .iter()
                .all(|word| word.metadata.as_ref().unwrap()["capture_source"] == "import")
        );
        assert!(
            first
                .iter()
                .all(|word| word.metadata.as_ref().unwrap()["timing"]["source"] == "provider_word")
        );
        let ids: std::collections::HashSet<_> = first
            .iter()
            .map(|word| word.id.as_deref().unwrap())
            .collect();
        assert_eq!(ids.len(), first.len());
    }

    #[test]
    fn mono_windows_preserve_known_audio_origin_and_default_to_unknown() {
        let mut meta: hypr_vault_read::SessionMeta = serde_json::from_value(serde_json::json!({
            "id": "s1", "title": "", "started_at": null, "ended_at": null,
            "created_at": "", "tags": []
        }))
        .unwrap();
        assert_eq!(native_session_audio(&meta), SessionAudio::default());
        for source in [AudioSource::Import, AudioSource::Recording] {
            meta.extra.insert(
                "audio".into(),
                serde_json::to_value(SessionAudio {
                    source,
                    layout: AudioLayout::MicSystem,
                })
                .unwrap(),
            );
            let audio = native_session_audio(&meta);
            assert_eq!(audio.source, source);
            assert_eq!(audio.layout, AudioLayout::Mixed);
        }
    }
}

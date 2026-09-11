---
name: qa-critical-ux
description: QA-test Loofah's critical macOS desktop experience before a release — onboarding and permissions, vault-backed notes, microphone reminders, recording, on-device transcription, imports, and optional summaries through local or API-key Intelligence providers. Use before cutting a stable release, after changes to capture/STT/enhance/notification/vault flows, or when asked to "QA the app".
---

# QA: Critical User Experience

Gate releases on the applicable checks below. Every check must pass or be
explicitly waived by the user; an unwaived failure blocks release.

## Current product boundaries

- Loofah has no account, cloud backend, or calendar connection flow.
- Transcription is on-device only. A downloaded model may transcribe live or
  after recording, depending on the model and selected languages.
- Intelligence is optional and separate from transcription. Summaries and
  generated titles can use LM Studio/Ollama or a configured remote provider.
- The vault files are the source of truth. Do not use a legacy database as
  evidence of success.

## Setup

1. Launch the app with `pnpm -F @hypr/desktop tauri:dev`; reuse a healthy
   running instance when possible.
2. Use a disposable or copied test vault. Do not reset or repoint the user's
   real vault merely to obtain a clean onboarding run.
3. Grant macOS Microphone, System Audio, and notification permissions. Use a
   real meeting app for microphone-detection and auto-stop checks.
4. Download and select the on-device transcription models required by the
   matrix. Wait for each model to report ready before testing it.
5. For summary runs, prepare one local Intelligence provider (LM Studio or
   Ollama) and one remote provider with an API key. A summary needs a meaningful
   transcript; use a spoken paragraph comfortably longer than 160 characters.
6. Record the app version, vault path, selected languages, STT model and mode,
   and Intelligence provider/model.

## Checklist

### 1. First run, permissions, and storage

Run this on a clean test profile when onboarding or storage behavior is in
scope.

- Complete the Welcome flow: grant Microphone and System Audio access, select
  the test vault, and start or select an on-device transcription-model
  download. Configure Intelligence or intentionally skip it.
- PASS when onboarding reaches the main app, the selected vault opens, the
  welcome note is usable, a queued model download continues, and the same vault
  and settings return after restart.
- Verify denied permissions lead to an actionable macOS-settings path rather
  than a dead end.

### 2. Create and persist a note

- Choose **New Note**, enter a title, and type distinctive content in **Note**.
- PASS when the editor opens immediately, the note appears in the sidebar
  timeline and search, and title/content survive switching notes and restarting
  the app.
- In the vault, confirm the session has `_meta.json` and `notes.md` with the
  expected content.

### 3. Meeting reminder and automatic controls

- In **Settings → Notifications**, enable **Microphone detection**, select a
  short detection delay, and start microphone use in a non-excluded meeting
  app.
- PASS when the **Are you in a meeting?** notification appears and **Start
  recording** creates a note and begins recording. Loofah must never begin
  recording before the user accepts the notification.
- Use **Always ignore** or the excluded-app control and verify the same app no
  longer prompts. Re-enable it and verify the setting persists.
- In **Settings → App → Meetings**, enable **Show floating bar** and **Stop when
  meeting ends**. PASS when the compact controls appear while recording and the
  active session stops and finalizes after the trigger app releases the
  microphone. Also verify manual **Stop** remains reliable.

### 4. Record and transcribe

- Create an empty note and click **Record**. Speak distinct sentences and play
  audio so microphone and system-audio capture both carry signal.
- PASS while recording when the recording state, elapsed time, and audio
  activity respond; the **Note** editor remains usable; and **Stop** works from
  the main window and floating controls.
- With a live-capable model and supported languages, PASS when words appear in
  **Transcript** during recording and persist after stop.
- With an after-recording model or a language combination that disables live
  transcription, PASS when the UI explains the fallback, recording continues,
  and **Transcribing**/**Finalizing** produces a complete transcript after
  stop. Batch transcripts should detect speakers; rename a speaker and verify
  the assignment persists.
- If live transcription stalls, the expected behavior is a warning while audio
  keeps recording, followed by batch repair after stop. Missing transcript text
  after finalization is a failure.
- Click **Resume** on the finished session, record another distinct passage,
  and stop again. PASS when the transcript is extended without duplication and
  any existing automatic summary is regenerated from the updated transcript.

### 5. Automatic summary and generated title

Repeat a sufficiently long transcript under each Intelligence configuration in
the matrix.

- Stop recording or finish an import.
- PASS with a configured provider when a **Summary** is generated
  automatically, reflects the transcript and typed note context, and an
  untitled session receives a useful generated title. Generation failures must
  be visible and retryable rather than silent.
- Regenerate once and, when templates are in scope, switch the summary template
  and regenerate. PASS when the selected format is used without damaging the
  transcript or note.
- PASS with no Intelligence model when recording and transcription still work
  and the empty Summary view offers setup instead of reporting a false
  generation failure.

### 6. Import smoke test

- On an empty note, use **Upload audio** with one supported audio file. PASS
  when it transcribes after import, shows speaker-separated output, and creates
  a summary only when Intelligence is configured.
- On another empty note, use **Upload transcript** with a VTT or SRT file. PASS
  when timed text appears in **Transcript**, no audio is invented, and an
  eligible transcript triggers a summary when Intelligence is configured.
- If import code changed, also use **Import Audio** from the empty screen with
  multiple files. PASS when each file becomes a note, the queue advances one at
  a time, failures are inspectable/retryable, and completion is reported.

## Required configuration matrix

| Area | Configuration | Expected outcome |
| --- | --- | --- |
| Transcription | Live-capable on-device model + supported language | Live words, persisted transcript |
| Transcription | After-recording model or unsupported live language combination | Explicit fallback, complete post-stop transcript, speaker detection |
| Intelligence | None | Notes, recording, and transcript work; Summary offers setup |
| Intelligence | LM Studio or Ollama | Local automatic summary and generated title |
| Intelligence | One API-key provider | Remote automatic summary and generated title |

STT is always on-device: do not invent an API-key STT pass. Do not require a
sign-in/sign-out pass. Record any unavailable configuration as an explicit
waiver rather than silently omitting it.

## Vault and runtime evidence

Prefer user-visible behavior, then corroborate it with logs and exact
app-owned vault files:

- `_meta.json` for title and session metadata
- `notes.md` for the editable note
- `transcript.json` for transcript words and speaker assignments
- `enhanced/<uuid>.md` for generated summaries
- `audio.mp3`, `audio.wav`, or `audio.ogg` plus `audio.peaks.json`
- vault-root `config.json` for persisted settings

Recordings are kept indefinitely unless the user explicitly deletes them.
Unknown and dot-prefixed session files are not app
content and must not be treated as evidence or modified.

The Tauri webview is not reachable through an in-app browser pane. Use
available screenshot/accessibility automation, but expect a human to handle OS
permission prompts, microphone/system audio, notification actions, and local
provider startup. On failure, capture the visible error, reproduction steps,
relevant logs, and the app-owned vault artifacts; do not query a retired
database.

## Reporting

Report a table with **area**, **configuration**, **PASS/FAIL/WAIVED**, and a
one-line evidence note. Include the recorded environment/configuration and a
short reproduction for every failure. List waivers explicitly; any other FAIL
blocks release.

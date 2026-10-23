use crate::{
    cli::{Command, LiveCommand},
    output,
};
use hypr_meeting_live::{self as live, Request, Response};
use std::path::Path;

pub async fn request(vault: &Path, request: Request) -> live::Result<Response> {
    if let Request::Nudge(input) = &request {
        live::normalize_nudge(&input.text)?;
    }
    let identity = live::Identity::new(live::cli_channel(), vault)?;
    live::call(identity, request).await
}

pub fn data(response: Response) -> serde_json::Value {
    match response {
        Response::Status(value) => serde_json::to_value(value),
        Response::Transcript(value) => serde_json::to_value(value),
        Response::Nudge(value) => serde_json::to_value(value),
        Response::Error(value) => serde_json::to_value(value),
    }
    .expect("live response is serializable")
}

pub async fn run(vault: &Path, command: Command, json: bool) -> crate::Result<u8> {
    let (name, input) = match command {
        Command::Nudge { text, recording_id } => (
            "nudge",
            Request::Nudge(live::NudgeInput { text, recording_id }),
        ),
        Command::Live {
            command: LiveCommand::Status,
        } => ("live.status", Request::Status),
        Command::Live {
            command:
                LiveCommand::Transcript {
                    recording_id,
                    cursor,
                },
        } => (
            "live.transcript",
            Request::Transcript(live::TranscriptInput {
                recording_id,
                cursor,
            }),
        ),
        _ => unreachable!(),
    };
    let response = request(vault, input).await?;
    if json {
        output::emit(&output::json(name, &data(response), None)?);
    } else {
        match response {
            Response::Status(status) => match status.active_meeting {
                Some(active) => output::emit(&format!(
                    "{}\nRecording: {}\nMeeting: {}\nLive transcription: {}\nNudges: {}",
                    active.title,
                    active.recording_id,
                    active.meeting_id,
                    active.live_transcription_available,
                    active.nudges_available
                )),
                None => output::emit("No active recording"),
            },
            Response::Nudge(receipt) => output::emit(&format!(
                "Nudge accepted: {} (recording {})",
                receipt.nudge_id, receipt.recording_id
            )),
            Response::Transcript(page) => {
                if page.reset {
                    output::emit("Snapshot: replace previous live context");
                }
                if page.truncated {
                    output::emit(
                        "Earlier context omitted; use sessions transcript for the complete saved transcript",
                    );
                }
                if !page.live_transcription_available {
                    output::emit("Live transcription is unavailable for this recording");
                }
                for change in page.changes {
                    match change {
                        live::Change::Upsert { word } => output::emit(&format!(
                            "[{}ms] {}: {} ({})",
                            word.start_ms, word.speaker_label, word.text, word.id
                        )),
                        live::Change::Remove { id } => {
                            output::emit(&format!("Remove corrected word: {id}"))
                        }
                    }
                }
                output::emit(&format!(
                    "Cursor: {}\nMore: {}\nState: {:?}",
                    page.cursor, page.has_more, page.state
                ));
            }
            Response::Error(error) => return Err(error.into()),
        }
    }
    Ok(0)
}

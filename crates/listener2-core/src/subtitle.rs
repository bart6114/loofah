use aspasia::{Subtitle as SubtitleTrait, TimedSubtitleFile, WebVttSubtitle};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Token {
    text: String,
    start_time: u64,
    end_time: u64,
    speaker: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Subtitle {
    tokens: Vec<Token>,
}

impl From<TimedSubtitleFile> for Subtitle {
    fn from(sub: TimedSubtitleFile) -> Self {
        let vtt: WebVttSubtitle = sub.into();

        let tokens = vtt
            .events()
            .iter()
            .map(|cue| Token {
                text: cue.text.clone(),
                start_time: i64::from(cue.start) as u64,
                end_time: i64::from(cue.end) as u64,
                speaker: cue.identifier.as_ref().filter(|s| !s.is_empty()).cloned(),
            })
            .collect();

        Self { tokens }
    }
}

pub fn parse_subtitle_from_path<P: AsRef<std::path::Path>>(
    path: P,
) -> std::result::Result<Subtitle, String> {
    let sub = TimedSubtitleFile::new(path.as_ref()).map_err(|e| e.to_string())?;
    Ok(sub.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_vtt_and_srt_without_modifying_the_source() {
        let dir = tempfile::tempdir().unwrap();
        for (extension, source) in [
            (
                "vtt",
                "WEBVTT\n\nAlice\n00:00:01.000 --> 00:00:02.500\nHello there\n",
            ),
            ("srt", "1\n00:00:01,000 --> 00:00:02,500\nHello there\n"),
        ] {
            let path = dir.path().join(format!("transcript.{extension}"));
            std::fs::write(&path, source).unwrap();
            let subtitle = parse_subtitle_from_path(&path).unwrap();
            assert_eq!(subtitle.tokens.len(), 1);
            let token = &subtitle.tokens[0];
            assert_eq!(token.text, "Hello there");
            assert_eq!(token.start_time, 1000);
            assert_eq!(token.end_time, 2500);
            if extension == "vtt" {
                assert_eq!(token.speaker.as_deref(), Some("Alice"));
            }
            assert_eq!(std::fs::read_to_string(path).unwrap(), source);
        }
    }
}

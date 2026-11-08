use serde::{Deserialize, Serialize};

pub const TRANSCRIPT_OVERVIEW_LABEL_LIMIT: usize = 4096;
pub const TRANSCRIPT_OVERVIEW_RESPONSE_LIMIT: usize = 256 * 1024;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct TranscriptOverview {
    pub has_words: bool,
    pub speaker_labels: Vec<String>,
    pub started_at: Option<f64>,
    pub ended_at: Option<f64>,
}

pub fn validate_transcript_overview(value: &TranscriptOverview) -> Result<(), &'static str> {
    if value.speaker_labels.len() > TRANSCRIPT_OVERVIEW_LABEL_LIMIT {
        return Err("Transcript overview has too many speaker labels");
    }
    let mut remaining = TRANSCRIPT_OVERVIEW_RESPONSE_LIMIT;
    let mut seen = std::collections::HashSet::new();
    for label in &value.speaker_labels {
        remaining = remaining
            .checked_sub(label.len())
            .ok_or("Transcript overview is too large")?;
        if label.is_empty() || !seen.insert(label.as_str()) {
            return Err("Transcript overview speaker labels must be nonempty and unique");
        }
    }
    if [value.started_at, value.ended_at]
        .into_iter()
        .flatten()
        .any(|time| !time.is_finite())
    {
        return Err("Transcript overview timestamps must be finite");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_preserves_explicit_times_and_raw_labels_without_inference() {
        let value = TranscriptOverview {
            has_words: false,
            speaker_labels: vec!["  person  ".into(), "é\nlabel".into()],
            started_at: Some(10.0),
            ended_at: Some(1.0),
        };
        assert!(validate_transcript_overview(&value).is_ok());
        assert_eq!(
            serde_json::from_str::<TranscriptOverview>(&serde_json::to_string(&value).unwrap())
                .unwrap(),
            value
        );
        assert_eq!(
            serde_json::to_value(TranscriptOverview::default()).unwrap(),
            serde_json::json!({"has_words":false,"speaker_labels":[],"started_at":null,"ended_at":null})
        );
    }

    #[test]
    fn malformed_or_unbounded_replies_fail_instead_of_truncating() {
        for labels in [
            vec!["".into()],
            vec!["a".into(), "a".into()],
            (0..=TRANSCRIPT_OVERVIEW_LABEL_LIMIT)
                .map(|i| i.to_string())
                .collect(),
            vec!["é".repeat(TRANSCRIPT_OVERVIEW_RESPONSE_LIMIT / 2 + 1)],
        ] {
            assert!(
                validate_transcript_overview(&TranscriptOverview {
                    speaker_labels: labels,
                    ..Default::default()
                })
                .is_err()
            );
        }
        assert!(
            validate_transcript_overview(&TranscriptOverview {
                started_at: Some(f64::NAN),
                ..Default::default()
            })
            .is_err()
        );
    }
}

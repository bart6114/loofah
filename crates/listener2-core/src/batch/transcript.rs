pub use hypr_transcript::batch::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, Value};

    fn timing(source: &str) -> Map<String, Value> {
        serde_json::from_value(serde_json::json!({ "timing": { "source": source } })).unwrap()
    }

    /// End-to-end against the real soniqo response builder: the words the
    /// desktop would persist for a soniqo batch result.
    #[test]
    fn soniqo_batch_response_maps_like_the_desktop() {
        let response = hypr_transcribe_soniqo::batch_response_from_text(
            hypr_transcribe_soniqo::SoniqoModel::ParakeetBatch,
            "hello world".to_string(),
            2.0,
        );

        let (words, hints) = words_and_hints_from_batch_response(&response);

        assert!(hints.is_empty());
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].text, " hello");
        assert_eq!(words[1].text, " world");
        assert_eq!(words[0].start_ms, 0.0);
        assert_eq!(words[0].end_ms, 400.0);
        assert_eq!(words[1].end_ms, 800.0);
        // soniqo stamps metadata.timing_source, which flows into every word.
        assert_eq!(words[0].metadata, Some(timing("synthetic_text")));
    }
}

use super::*;
use serde_json::json;

fn binding() -> CaptureLiveBinding {
    CaptureLiveBinding {
        capture_id: uuid::Uuid::from_u128(1).to_string(),
        generation_id: uuid::Uuid::from_u128(2).to_string(),
        capture_fingerprint: "a".repeat(64),
        session_id: "note".into(),
        format: hypr_loofah_protocol::CaptureFormat::stereo(),
    }
}
fn page(sequence: u64, words: Vec<(&str, &str)>, replaced: Vec<&str>) -> CaptureLiveCaptionPage {
    let binding = binding();
    serde_json::from_value(json!({
        "snapshot": { "capture_id":binding.capture_id,"capture_fingerprint":binding.capture_fingerprint,"generation_id":binding.generation_id,"session_id":binding.session_id,"state":"running","failure":null,"admitted_end_frame":10,"input_consumed_end_frame":10,"completed_through_frame":null,"next_caption_sequence":sequence+1,"result_available":false,"publication":null },
        "captions":[{ "generation_id":binding.generation_id,"sequence":sequence,"physical_channel":0,"role":"microphone","new_words":words.into_iter().enumerate().map(|(index,(id,text))| json!({"id":id,"text":text,"start_ms":index,"end_ms":index+1,"channel":0,"state":"final"})).collect::<Vec<_>>(),"partials":[],"replaced_ids":replaced,"languages":[] }],
        "next_cursor":null,
    })).unwrap()
}

#[test]
fn caption_tail_deduplicates_sequences_ids_and_replacements_without_crossing_generations() {
    let mut preview = DeviceCaptionPreview::new(binding());
    let first = page(0, vec![("one", "hello"), ("two", "world")], vec![]);
    assert!(preview.page(first.clone()));
    assert!(preview.page(first));
    assert_eq!(preview.words.len(), 2);
    assert_eq!(preview.next_sequence, 1);
    assert!(preview.page(page(
        1,
        vec![("two", "Loofah"), ("two", "everyone")],
        vec!["one"]
    )));
    assert_eq!(preview.words.len(), 1);
    assert_eq!(preview.words[0].text, "everyone");
    let before = preview.clone();
    let mut foreign = page(2, vec![("other", "private")], vec![]);
    foreign.snapshot.generation_id = uuid::Uuid::new_v4().to_string();
    assert!(!preview.page(foreign));
    assert_eq!(preview, before);
    assert!(!preview.page(page(3, vec![("gap", "later")], vec![])));
    assert_eq!(preview, before);
    assert!(preview.page(page(2, vec![("repair", "recovered")], vec![])));
    assert_eq!(preview.next_sequence, 3);
}

#[test]
fn caption_tail_bounds_words_partials_and_utf8_bytes_while_preserving_channel_roles() {
    let mut preview = DeviceCaptionPreview::new(binding());
    let mut large = page(0, vec![], vec![]);
    for index in 0..300 {
        large.captions[0].new_words.push(serde_json::from_value(json!({"id":format!("word-{index}"),"text":"é".repeat(64),"start_ms":index,"end_ms":index+1,"channel":0,"state":"final"})).unwrap());
        large.captions[0].partials.push(
            serde_json::from_value(
                json!({"text":"partial".repeat(16),"start_ms":index,"end_ms":index+1,"channel":0}),
            )
            .unwrap(),
        );
    }
    assert!(preview.page(large));
    assert!(preview.words.len() <= MAX_WORDS);
    assert!(preview.partials.len() <= MAX_PARTIALS);
    assert!(
        preview
            .words
            .iter()
            .chain(&preview.partials)
            .map(|word| word.text.len())
            .sum::<usize>()
            <= MAX_TEXT_BYTES
    );
    assert!(
        preview
            .words
            .iter()
            .chain(&preview.partials)
            .all(|word| word.role == CaptureChannel::Microphone && word.channel == 0)
    );
    let mut speaker = page(1, vec![("speaker", "reply")], vec![]);
    speaker.captions[0].physical_channel = 1;
    speaker.captions[0].role = CaptureChannel::SystemAudio;
    speaker.captions[0].new_words[0].channel = 1;
    speaker.captions[0].new_words[0].start_ms = 400;
    speaker.captions[0].new_words[0].end_ms = 401;
    assert!(preview.page(speaker));
    assert!(!preview.partials.is_empty());
    assert!(
        preview
            .words
            .iter()
            .any(|word| word.role == CaptureChannel::SystemAudio && word.channel == 1)
    );
}

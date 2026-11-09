use super::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviousRecord {
    schema_version: u32,
    vault_id: String,
    member: String,
    request_id: String,
    request: TranscriptionJobRequest,
    snapshot: TranscriptionJobSnapshot,
    action: String,
    uncertain: bool,
    source: Option<serde_json::Value>,
    result: Option<serde_json::Value>,
}
#[tokio::test]
async fn ordinary_batch_bytes_match_previous_reader_and_marked_admission_is_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("jobs");
    let journal = Journal::open(
        root.clone(),
        crate::events::EventHub::new("vault".into(), 16),
        TranscriptionLimits::default(),
        None,
    )
    .await
    .unwrap();
    let request = TranscriptionJobRequest {
        session_id: "note".into(),
        model: "base".into(),
        languages: vec![],
        keywords: vec![],
        expected_recording_sha256: "a".repeat(64),
        expected_transcript: AbsentTranscriptExpectation::Absent,
    };
    let member = uuid::Uuid::new_v4().to_string();
    let snapshot = journal
        .submit(
            member.clone(),
            uuid::Uuid::new_v4().to_string(),
            request.clone(),
        )
        .await
        .unwrap();
    let bytes = std::fs::read(root.join(format!("{}.json", snapshot.id))).unwrap();
    let previous: PreviousRecord = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(serde_json::to_vec(&previous).unwrap(), bytes);
    let origin:crate::capture_processing::CaptureChildOrigin=serde_json::from_value(serde_json::json!({"capture_id":uuid::Uuid::new_v4().to_string(),"intent_id":uuid::Uuid::new_v4().to_string(),"capture_fingerprint":"b".repeat(64),"model_sha256":"c".repeat(64),"request_sha256":hash(&serde_json::to_vec(&request).unwrap())})).unwrap();
    let snapshot = journal
        .submit_with_origin(
            member,
            uuid::Uuid::new_v4().to_string(),
            request,
            Some(origin.clone()),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.state, TranscriptionJobState::Queued);
    let bytes = std::fs::read(root.join(format!("{}.json", snapshot.id))).unwrap();
    assert_eq!(
        serde_json::from_slice::<Record>(&bytes)
            .unwrap()
            .capture_processing,
        Some(origin)
    );
    assert!(
        serde_json::from_slice::<PreviousRecord>(&bytes)
            .unwrap_err()
            .to_string()
            .contains("capture_processing")
    );
}

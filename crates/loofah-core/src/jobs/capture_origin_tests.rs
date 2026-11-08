use super::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviousRecord {
    schema_version: u32,
    vault_id: String,
    namespace: String,
    request_id: String,
    request: SummaryJobRequest,
    snapshot: ProcessingJobSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body_applied: Option<serde_json::Value>,
}
#[tokio::test]
async fn ordinary_summary_bytes_match_previous_reader_and_marked_admission_is_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("jobs");
    let journal = JobJournal::open(root.clone(), EventHub::new("vault".into(), 16))
        .await
        .unwrap();
    let request = SummaryJobRequest {
        session_id: "note".into(),
        target: DocumentTarget::Summary,
        expected_revision: "a".repeat(64),
        title_policy: TitlePolicy::Preserve,
    };
    let id = uuid::Uuid::new_v4().to_string();
    journal
        .submit("member".into(), id.clone(), request.clone())
        .await
        .unwrap();
    let bytes = std::fs::read(root.join(format!("{}.json", key("member", &id)))).unwrap();
    let previous: PreviousRecord = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(serde_json::to_vec(&previous).unwrap(), bytes);
    let origin:crate::capture_processing::CaptureChildOrigin=serde_json::from_value(serde_json::json!({"capture_id":uuid::Uuid::new_v4().to_string(),"intent_id":uuid::Uuid::new_v4().to_string(),"capture_fingerprint":"b".repeat(64),"model_sha256":"c".repeat(64),"request_sha256":crate::transcription_jobs::ledger::hash(&serde_json::to_vec(&request).unwrap())})).unwrap();
    let marked = uuid::Uuid::new_v4().to_string();
    let snapshot = journal
        .submit_with_origin(
            "member".into(),
            marked.clone(),
            request,
            Some(origin.clone()),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.state, ProcessingJobState::Queued);
    let bytes = std::fs::read(root.join(format!("{}.json", key("member", &marked)))).unwrap();
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

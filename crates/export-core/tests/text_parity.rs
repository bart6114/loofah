use export_core::{ExportInput, ExportLabels, TextFormat, render_text};

#[test]
fn desktop_fixture_preserves_text_format_contract() {
    let input: ExportInput =
        serde_json::from_str(include_str!("fixtures/desktop-export.json")).unwrap();
    let labels = ExportLabels::default();
    for (format, expected) in [
        (
            TextFormat::Md,
            "# Démo 👋\n- Created: Friday, September 18, 2026 at 12:00 AM\n- Participants: Zoë, 李\n- Duration: 2m\n\n## Note\n## Café\n\n- **naïve** [link](https://example.com)\n- 你好 👋\n\n## Summary\n**Décision**: ship `v2`.\n\n## Transcript\nZoë: Bonjour — 你好.\n\nSpeaker 1: Unknown speaker.\n\nUnlabelled.",
        ),
        (
            TextFormat::Txt,
            "Démo 👋\n=======\nFriday, September 18, 2026 at 12:00 AM\nParticipants: Zoë, 李\nDuration: 2m\n\nNote\n----\nCafé\n• naïve link (https://example.com)\n• 你好 👋\n\nSummary\n-------\nDécision: ship v2.\n\nTranscript\n----------\nZoë: Bonjour — 你好.\n\nSpeaker 1: Unknown speaker.\n\nUnlabelled.",
        ),
        (
            TextFormat::Org,
            "#+TITLE: Démo 👋\n#+DATE: Friday, September 18, 2026 at 12:00 AM\n\n* Metadata\n- Created :: Friday, September 18, 2026 at 12:00 AM\n- Participants :: Zoë, 李\n- Duration :: 2m\n\n* Note\n** Café\n\n- *naïve* [[https://example.com][link]]\n- 你好 👋\n\n* Summary\n*Décision*: ship ~v2~.\n\n* Transcript\nZoë: Bonjour — 你好.\n\nSpeaker 1: Unknown speaker.\n\nUnlabelled.",
        ),
    ] {
        assert_eq!(render_text(&input, format, &labels), expected);
    }
}

#[test]
fn empty_sections_are_omitted_and_labels_are_localizable() {
    let input = ExportInput {
        enhanced_md: " \n".into(),
        note_md: None,
        transcript: None,
        metadata: None,
        attachments: vec![],
    };
    let labels = ExportLabels {
        untitled: "Sans titre".into(),
        ..Default::default()
    };
    assert_eq!(render_text(&input, TextFormat::Md, &labels), "# Sans titre");
    assert_eq!(
        render_text(&input, TextFormat::Txt, &labels),
        "Sans titre\n=========="
    );
    assert_eq!(
        render_text(&input, TextFormat::Org, &labels),
        "#+TITLE: Sans titre\n\n* Metadata"
    );
}

use std::ops::Range;

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

fn errors(expected: &[String], actual: &[String]) -> usize {
    let mut row: Vec<_> = (0..=actual.len()).collect();
    for (i, expected) in expected.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, actual) in actual.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (previous + usize::from(expected != actual))
                .min(row[j] + 1)
                .min(above + 1);
            previous = above;
        }
    }
    row[actual.len()]
}

fn decode(
    loaded: &hypr_whisper_local::LoadedWhisper,
    language: &str,
    samples: &[f32],
    ranges: &[Range<usize>],
    pad: bool,
) -> String {
    let params = super::parse_listen_params(&format!("language={language}")).unwrap();
    let mut model = super::build_model(loaded, &params).unwrap();
    let mut text = Vec::new();
    for range in ranges {
        if !pad && range.len() < hypr_whisper_local::MIN_TRANSCRIBE_SAMPLES {
            continue;
        }
        let segments = super::transcribe_chunk(
            &mut model,
            &samples[range.clone()],
            range.start as f64 / 16000.0,
        )
        .unwrap();
        for segment in segments {
            assert!(segment.start >= range.start as f64 / 16000.0);
            assert!(segment.start + segment.duration <= range.end as f64 / 16000.0 + 1e-6);
            text.push(segment.text);
        }
    }
    text.join(" ")
}

#[test]
#[ignore = "requires LOOFAH_WHISPER_MODEL; compares fixed English and Dutch boundary fixtures"]
fn fixed_english_and_dutch_boundaries_do_not_regress() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/boundaries");
    let reference: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("reference.json")).unwrap()).unwrap();
    let loaded = hypr_whisper_local::LoadedWhisper::builder()
        .model_path(std::env::var("LOOFAH_WHISPER_MODEL").unwrap())
        .use_gpu(false)
        .build()
        .unwrap();
    let mut baseline_errors = 0;
    let mut after_errors = 0;
    let mut report = serde_json::Map::new();
    let mut missing_boundary_words = Vec::new();
    for (name, fixture) in reference.as_object().unwrap() {
        let language = fixture["language"].as_str().unwrap_or(name);
        let pcm =
            hypr_audio_utils::PcmFile::prepare(root.join(format!("{name}.wav")), || false).unwrap();
        let pcm = &pcm.descriptor;
        let samples = pcm.reader().unwrap().channel(0, 0..pcm.frames).unwrap();
        assert_eq!(pcm.frames, fixture["frames"].as_u64().unwrap() as usize);
        assert!(samples.len() > 25 * 16000);
        let legacy: Vec<_> = (0..samples.len())
            .step_by(25 * 16000)
            .map(|start| start..(start + 25 * 16000).min(samples.len()))
            .collect();
        // Force a long selected speech span so native inference exercises the changed cuts.
        let selected = [hypr_audio_chunking::AudioChunk {
            sample_start: 0,
            sample_end: samples.len(),
            samples: Vec::new(),
        }];
        let chunks = super::packing::pack_ranges(samples.len(), &selected, None, |range| {
            Ok::<_, std::convert::Infallible>(samples[range].to_vec())
        })
        .unwrap();
        let current: Vec<_> = chunks
            .iter()
            .map(|c| c.sample_start..c.sample_end)
            .collect();
        assert_eq!(current.len(), legacy.len());
        if let Some(anchor) = fixture["boundary_word_range_samples"].as_array() {
            let start = anchor[0].as_u64().unwrap() as usize;
            let end = anchor[1].as_u64().unwrap() as usize;
            assert!(start < 400000 && end > 400000);
            assert!(
                current
                    .iter()
                    .all(|range| range.end <= start || range.end >= end),
                "{name}: cut inside the boundary word"
            );
        }
        eprintln!("Checking {name}: {legacy:?} -> {current:?}");
        let baseline = decode(&loaded, language, &samples, &legacy, false);
        let after = decode(&loaded, language, &samples, &current, true);
        let expected = words(fixture["text"].as_str().unwrap());
        let baseline_words = words(&baseline);
        let after_words = words(&after);
        let old_errors = errors(&expected, &baseline_words);
        let new_errors = errors(&expected, &after_words);
        eprintln!("{name}: word errors {old_errors} -> {new_errors}");
        baseline_errors += old_errors;
        after_errors += new_errors;
        for word in fixture["boundary_words"].as_array().unwrap() {
            let word = word.as_str().unwrap();
            let expected_count = expected.iter().filter(|w| w.as_str() == word).count();
            let count = after_words.iter().filter(|w| w.as_str() == word).count();
            if count != expected_count {
                missing_boundary_words.push(format!(
                    "{name}: {word}: expected {expected_count}, found {count}"
                ));
            }
        }
        report.insert(name.clone(), serde_json::json!({"baseline": baseline, "after": after, "reference_words": expected.len(), "baseline_errors": old_errors, "after_errors": new_errors, "baseline_ranges": legacy.iter().map(|r| [r.start, r.end]).collect::<Vec<_>>(), "after_ranges": current.iter().map(|r| [r.start, r.end]).collect::<Vec<_>>() }));
    }
    let report = serde_json::to_string_pretty(&report).unwrap();
    println!("{report}");
    if let Ok(path) = std::env::var("LOOFAH_BOUNDARY_REPORT") {
        std::fs::write(path, &report).unwrap();
    }
    assert!(
        missing_boundary_words.is_empty(),
        "{missing_boundary_words:?}"
    );
    assert!(
        after_errors <= baseline_errors,
        "aggregate errors increased from {baseline_errors} to {after_errors}"
    );
}

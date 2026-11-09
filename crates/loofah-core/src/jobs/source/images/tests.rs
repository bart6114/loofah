use super::*;

fn png() -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([80, 10, 40])))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

#[test]
fn reference_parsing_matches_json_and_legacy_asset_paths_without_accepting_remote_urls() {
    let raw = r#"{"type":"doc","content":[{"type":"image","attrs":{"attachmentId":"chart.png","src":"asset://localhost/vault/attachments/chart.png"}},{"type":"fileAttachment","attrs":{"mimeType":"image/png","path":"attachments/screen%20shot.png"}},{"type":"image","attrs":{"src":"https://example.com/attachments/private.png"}},{"type":"fileAttachment","attrs":{"src":"attachments/report.pdf","mimeType":"application/pdf"}}]}"#;
    let refs = references(&[raw]).unwrap();
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].attachment_id.as_deref(), Some("chart.png"));
    assert_eq!(refs[1].filename.as_deref(), Some("screen shot.png"));
    let refs = references(&["![first](asset://localhost/vault/attachments/chart.png)\n![second](<attachments/screen%20shot.png>)\n![web](https://example.com/attachments/private.png)"]).unwrap();
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[1].filename.as_deref(), Some("screen shot.png"));
    assert!(
        references(&["![unknown](loose.png)\n![private](attachments/.secret.png)"])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn data_urls_and_budgets_preserve_small_images_and_reject_invalid_or_oversized_data() {
    let original = png();
    let src = format!("data:image/png;base64,{}", STANDARD.encode(&original));
    let source = data_url(&src).unwrap();
    assert_eq!(source.bytes, original);
    let result = budget(source, MAX_IMAGE_BYTES, &CancellationToken::new()).unwrap();
    assert_eq!(result.bytes, original);
    assert_eq!(result.mime_type, "image/png");
    assert_eq!(data_url(src.trim_end_matches('=')).unwrap().bytes, original);
    assert!(data_url("data:text/html;base64,PHNjcmlwdD4=").is_none());
    assert!(data_url("data:image/png;base64,not-base64").is_none());
    assert!(
        data_url(&format!(
            "data:image/png;base64,{}",
            "A".repeat(MAX_SOURCE_BYTES.div_ceil(3) * 4 + 4)
        ))
        .is_none()
    );
    assert!(
        budget(
            SummaryImage {
                bytes: vec![0; 20],
                mime_type: "image/png".into()
            },
            MAX_IMAGE_BYTES,
            &CancellationToken::new()
        )
        .is_none()
    );
}

#[test]
fn compression_is_portable_bounded_and_does_not_modify_source_bytes() {
    let mut value = 7u32;
    let image = image::RgbImage::from_fn(256, 256, |_, _| {
        let mut channels = [0; 3];
        for channel in &mut channels {
            value = value.wrapping_mul(1664525).wrapping_add(1013904223);
            *channel = (value >> 24) as u8;
        }
        image::Rgb(channels)
    });
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let original = bytes.into_inner();
    assert!(original.len() > MAX_IMAGE_BYTES);
    let output = budget(
        SummaryImage {
            bytes: original.clone(),
            mime_type: "image/png".into(),
        },
        MAX_IMAGE_BYTES,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(output.bytes.len() <= MAX_IMAGE_BYTES);
    assert_eq!(output.mime_type, "image/jpeg");
    let decoded = image::load_from_memory(&output.bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (256, 256));
    assert_eq!(image::load_from_memory(&original).unwrap().width(), 256);
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(
        budget(
            SummaryImage {
                bytes: original,
                mime_type: "image/png".into()
            },
            MAX_IMAGE_BYTES,
            &cancelled
        )
        .is_none()
    );
}

#[test]
fn jpeg_compression_applies_exif_orientation_before_removing_metadata() {
    let image = image::RgbImage::from_pixel(2, 3, image::Rgb([30, 80, 140]));
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode_image(&image)
        .unwrap();
    let mut exif =
        b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0".to_vec();
    exif.extend_from_slice(&[0; 1024]);
    let mut bytes = jpeg[..2].to_vec();
    bytes.extend_from_slice(&[0xff, 0xe1]);
    bytes.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    bytes.extend_from_slice(&exif);
    bytes.extend_from_slice(&jpeg[2..]);
    assert!(bytes.len() > 1024);
    let output = budget(
        SummaryImage {
            bytes,
            mime_type: "image/jpeg".into(),
        },
        1024,
        &CancellationToken::new(),
    )
    .unwrap();
    let decoded = image::load_from_memory(&output.bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (3, 2));
    assert!(output.bytes.len() <= 1024);
}

#[test]
fn oversized_decoded_images_and_reference_work_are_rejected_before_large_allocations() {
    let wide = image::RgbImage::from_pixel(16_385, 1, image::Rgb([50, 50, 50]));
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(wide)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let bytes = bytes.into_inner();
    assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 16_385);
    assert!(
        budget(
            SummaryImage {
                bytes,
                mime_type: "image/png".into()
            },
            MAX_IMAGE_BYTES,
            &CancellationToken::new()
        )
        .is_none()
    );
    let text = "![ref](attachments/chart.png)\n".repeat(MAX_REFERENCES + 1);
    assert_eq!(
        references(&[&text]).err().unwrap(),
        ProcessingJobFailure::ResourceLimit
    );
    assert_eq!(
        order(20),
        [
            0, 2, 4, 6, 8, 11, 13, 15, 17, 19, 1, 3, 5, 7, 9, 10, 12, 14, 16, 18
        ]
    );
}

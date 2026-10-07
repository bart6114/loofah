use std::collections::HashMap;
use std::path::Path;

use crate::{Error, ExportInput};

const SUPPORTED_IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "svg", "webp"];

pub fn export_pdf(path: impl AsRef<Path>, input: impl Into<ExportInput>) -> Result<(), Error> {
    std::fs::write(path.as_ref(), render_pdf(&input.into())?)?;
    Ok(())
}

pub fn render_pdf(input: &ExportInput) -> Result<Vec<u8>, Error> {
    let (images, files) = load_attachment_images(input);
    let typst_content = crate::typst::build_typst_content(input, &images);
    crate::typst::compile_to_pdf(&typst_content, files)
}

// Maps each readable image attachment to a virtual path served to the typst
// compiler. Unreadable files and formats typst can't render are skipped so a
// broken attachment never fails the whole export.
fn load_attachment_images(
    input: &ExportInput,
) -> (HashMap<String, String>, HashMap<String, Vec<u8>>) {
    let mut images = HashMap::new();
    let mut files = HashMap::new();

    for (index, attachment) in input.attachments.iter().enumerate() {
        let extension = Path::new(&attachment.path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        let Some(extension) = extension else {
            continue;
        };
        if !SUPPORTED_IMAGE_EXTENSIONS.contains(&extension.as_str()) {
            continue;
        }

        let Ok(bytes) = std::fs::read(&attachment.path) else {
            continue;
        };

        let vpath = format!("/att-{}.{}", index, extension);
        images.insert(attachment.src.clone(), vpath.clone());
        files.insert(vpath, bytes);
    }

    (images, files)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::ExportInput;

    const ONE_PX_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 252, 207, 192, 80,
        15, 0, 4, 133, 1, 128, 132, 169, 140, 33, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    #[test]
    fn pdf_compiles_with_embedded_image() {
        let input = ExportInput {
            enhanced_md: String::new(),
            note_md: Some("Some text\n\n![photo](attachments/photo.png)".to_string()),
            transcript: None,
            metadata: None,
            attachments: vec![],
        };

        let images = HashMap::from([(
            "attachments/photo.png".to_string(),
            "/att-0.png".to_string(),
        )]);
        let files = HashMap::from([("/att-0.png".to_string(), ONE_PX_PNG.to_vec())]);

        let content = crate::typst::build_typst_content(&input, &images);
        assert!(content.contains("#image(\"/att-0.png\", width: 80%)"));

        let pdf = crate::typst::compile_to_pdf(&content, files).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
    }

    #[test]
    fn pdf_loads_portable_images_and_skips_missing_or_non_image_attachments() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("photo é.PNG");
        std::fs::write(&image, ONE_PX_PNG).unwrap();
        let document = dir.path().join("document.pdf");
        std::fs::write(&document, b"not an image").unwrap();
        let src = "attachments/photo%20%C3%A9.PNG";
        let input = ExportInput {
            enhanced_md: String::new(),
            note_md: Some(format!("![Photo]({src})")),
            transcript: None,
            metadata: None,
            attachments: vec![
                crate::ExportAttachment {
                    src: src.into(),
                    path: image.to_string_lossy().into_owned(),
                },
                crate::ExportAttachment {
                    src: "attachments/document.pdf".into(),
                    path: document.to_string_lossy().into_owned(),
                },
                crate::ExportAttachment {
                    src: "attachments/missing.png".into(),
                    path: dir
                        .path()
                        .join("missing.png")
                        .to_string_lossy()
                        .into_owned(),
                },
            ],
        };
        let (images, files) = super::load_attachment_images(&input);
        assert_eq!(images.len(), 1);
        assert_eq!(files[&images[src]], ONE_PX_PNG);
        let typst = crate::typst::build_typst_content(&input, &images);
        assert!(typst.contains("#image(\"/att-0.png\", width: 80%)"));
        assert!(super::render_pdf(&input).unwrap().starts_with(b"%PDF"));
    }

    #[test]
    fn pdf_compiles_with_markdown_table() {
        let input = ExportInput {
            enhanced_md: "| Household penetration | Subscribers | Annual gross billings |\n| :--- | ---: | ---: |\n| 0.01% | 4,767 | €1.71m |\n| 1.00% | **476,720** | €171.05m |"
                .to_string(),
            note_md: None,
            transcript: None,
            metadata: None,
            attachments: vec![],
        };

        let content = crate::typst::build_typst_content(&input, &HashMap::new());
        let pdf = crate::typst::compile_to_pdf(&content, HashMap::new()).unwrap();

        assert!(pdf.starts_with(b"%PDF"));
    }
}

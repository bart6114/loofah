mod error;
mod export;
mod text;
mod types;
mod typst;

pub use error::{Error, Result};
pub use export::{export_pdf, render_pdf, render_pdf_with_labels};
pub use text::render_text;
pub use types::*;

/// Portable image references actually present in the selected export documents.
pub fn referenced_images(input: &ExportInput) -> Vec<String> {
    let mut images = std::collections::BTreeSet::new();
    for markdown in std::iter::once(input.enhanced_md.as_str()).chain(input.note_md.as_deref()) {
        for event in pulldown_cmark::Parser::new(markdown) {
            if let pulldown_cmark::Event::Start(pulldown_cmark::Tag::Image { dest_url, .. }) = event
            {
                images.insert(dest_url.into_string());
            }
        }
    }
    images.into_iter().collect()
}

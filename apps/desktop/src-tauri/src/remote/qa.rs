pub(super) fn fixture_enabled(identifier: &str) -> bool {
    cfg!(feature = "dev") && fixture_identifier(identifier)
}

fn fixture_identifier(identifier: &str) -> bool {
    matches!(
        identifier,
        "io.loofah.remote-qa" | "io.loofah.remote-qa-a" | "io.loofah.remote-qa-b"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_qa_trust_requires_dev_and_an_exact_disposable_bundle() {
        for identifier in [
            "io.loofah.remote-qa",
            "io.loofah.remote-qa-a",
            "io.loofah.remote-qa-b",
        ] {
            assert!(fixture_identifier(identifier));
            assert_eq!(fixture_enabled(identifier), cfg!(feature = "dev"));
        }
        for identifier in [
            "io.loofah",
            "io.loofah.dev",
            "io.loofah.staging",
            "io.loofah.separation-qa",
            "io.loofah.remote-qa-c",
            "io.loofah.remote-qa-a.extra",
            "IO.LOOFAH.REMOTE-QA-A",
            "io.loofah.remote-qa-a ",
            "",
        ] {
            assert!(!fixture_identifier(identifier));
            assert!(!fixture_enabled(identifier));
        }
    }
}

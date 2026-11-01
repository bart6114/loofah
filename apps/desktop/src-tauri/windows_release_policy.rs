pub fn validate(
    mode: &str,
    updater_key: bool,
    signer_command: bool,
    signer_identity: bool,
    configured_signer: bool,
) -> Result<(), &'static str> {
    if !updater_key {
        return Err("Stable Windows builds require TAURI_SIGNING_PRIVATE_KEY");
    }
    match mode {
        "unsigned" if signer_command || configured_signer => {
            Err("Unsigned Windows builds must not configure publisher signing")
        }
        "unsigned" => Ok(()),
        "azure" if signer_command && signer_identity && configured_signer => Ok(()),
        "azure" => {
            Err("Azure Windows builds require a signing command, identity, and Tauri signer")
        }
        _ => Err("WINDOWS_SIGNING_MODE must be unsigned or azure"),
    }
}

#[cfg(test)]
mod tests {
    use super::validate;

    #[test]
    fn unsigned_release_needs_no_publisher_credentials() {
        assert_eq!(validate("unsigned", true, false, false, false), Ok(()));
    }

    #[test]
    fn updater_signing_is_required_in_both_modes() {
        for mode in ["unsigned", "azure"] {
            assert!(validate(mode, false, true, true, true).is_err());
        }
    }

    #[test]
    fn azure_requires_every_signing_component() {
        assert_eq!(validate("azure", true, true, true, true), Ok(()));
        for (command, identity, config) in [
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            assert!(validate("azure", true, command, identity, config).is_err());
        }
    }

    #[test]
    fn invalid_or_inconsistent_mode_fails() {
        assert!(validate("", true, false, false, false).is_err());
        assert!(validate("typo", true, true, true, true).is_err());
        assert!(validate("unsigned", true, true, false, false).is_err());
        assert!(validate("unsigned", true, false, false, true).is_err());
    }
}

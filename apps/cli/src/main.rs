use std::{
    ffi::{OsStr, OsString},
    process::ExitCode,
};

use clap::Parser;
use clap::error::ErrorKind;
use loof_cli::Args;

#[tokio::main]
async fn main() -> ExitCode {
    let argv: Vec<OsString> = std::env::args_os().collect();
    let json = argv.iter().any(|arg| arg == "--json");
    if should_warn_about_legacy_command(&argv) {
        eprintln!(
            "warning: `loof meetings` is deprecated and will be removed; use `loof sessions` instead"
        );
    }
    let args = match Args::try_parse_from(argv) {
        Ok(args) => args,
        Err(error) => {
            let exit_code = error.exit_code();
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) || !json
            {
                let _ = error.print();
            } else {
                eprintln!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "schema_version": loof_cli::JSON_SCHEMA_VERSION,
                        "error": {
                            "code": "invalid_arguments",
                            "message": error.to_string(),
                            "exit_code": exit_code,
                        }
                    }))
                    .expect("argument error response is always serializable")
                );
            }
            return ExitCode::from(exit_code as u8);
        }
    };

    match loof_cli::run(args).await {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            if json {
                eprintln!("{}", error.to_json());
            } else {
                eprintln!("error: {error}");
            }
            ExitCode::from(error.exit_code())
        }
    }
}

fn should_warn_about_legacy_command(argv: &[OsString]) -> bool {
    !argv.iter().any(|arg| arg == "--json") && is_legacy_meeting_command(argv)
}

fn is_legacy_meeting_command(argv: &[OsString]) -> bool {
    let mut iter = argv.iter().skip(1);

    while let Some(arg) = iter.next() {
        if arg == OsStr::new("--base") {
            iter.next();
            continue;
        }
        if arg == OsStr::new("--vault-path") || arg == OsStr::new("--profile") {
            iter.next();
            continue;
        }
        if arg == OsStr::new("--json") {
            continue;
        }
        if let Some(arg) = arg.to_str() {
            if arg.starts_with("--base=")
                || arg.starts_with("--vault-path=")
                || arg.starts_with("--profile=")
            {
                continue;
            }
            if arg.starts_with('-') {
                continue;
            }
            return arg == "meetings";
        }
        return false;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn detects_legacy_command_after_global_options() {
        assert!(is_legacy_meeting_command(&argv(&[
            "loof",
            "--vault-path",
            "/tmp/vault",
            "--json",
            "meetings",
            "list",
        ])));
        assert!(is_legacy_meeting_command(&argv(&[
            "loof",
            "--base=/tmp/vault",
            "meetings",
            "list",
        ])));
        assert!(is_legacy_meeting_command(&argv(&[
            "loof",
            "--profile",
            "company",
            "meetings",
            "list"
        ])));
        assert!(is_legacy_meeting_command(&argv(&[
            "loof",
            "--profile=company",
            "meetings",
            "list"
        ])));
        assert!(!is_legacy_meeting_command(&argv(&[
            "loof",
            "--profile",
            "meetings",
            "sessions",
            "list"
        ])));
        assert!(!is_legacy_meeting_command(&argv(&[
            "loof", "sessions", "list",
        ])));
        assert!(!should_warn_about_legacy_command(&argv(&[
            "loof", "--json", "meetings", "list",
        ])));
        assert!(should_warn_about_legacy_command(&argv(&[
            "loof", "meetings", "list",
        ])));
    }

    #[cfg(unix)]
    #[test]
    fn tolerates_non_utf8_arguments() {
        use std::os::unix::ffi::OsStringExt;

        let argv = vec![
            OsString::from("loof"),
            OsString::from("import"),
            OsString::from_vec(vec![0xff]),
        ];

        assert!(!is_legacy_meeting_command(&argv));
    }
}

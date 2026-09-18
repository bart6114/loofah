use super::sync_error;
use crate::Result;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn label(directory: &Path) -> Result<String> {
    let binding = directory
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| sync_error("invalid service binding"))?;
    Ok(format!("io.loofah.sync.staging.{}", &binding[..32]))
}
fn run(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program).args(args).output().map_err(|_| {
        sync_error("service manager unavailable; run sync watch under an external supervisor")
    })?;
    if !output.status.success() {
        return Err(sync_error(
            "service manager failed; use sync watch under an external supervisor",
        ));
    }
    Ok(())
}
fn text(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| sync_error("service paths must be UTF-8"))
}
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn unit(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}
fn uid() -> Result<String> {
    let output = Command::new("id").arg("-u").output().map_err(sync_error)?;
    if !output.status.success() {
        return Err(sync_error("cannot determine service user"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub fn start(vault: &Path, directory: &Path, unlock: Option<&Path>) -> Result<()> {
    let backend = fs::read(directory.join("credential-backend.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    if backend
        .as_ref()
        .is_some_and(|value| value["backend"] == "encrypted_file")
        && unlock.is_none()
    {
        return Err(sync_error(
            "background encrypted credentials require --unlock-file; use watch for interactive unlocking",
        ));
    }
    let label = label(directory)?;
    let executable = directory.join("bin/loof-staging");
    fs::create_dir_all(executable.parent().unwrap()).map_err(sync_error)?;
    let temporary = executable.with_extension("new");
    fs::copy(std::env::current_exe().map_err(sync_error)?, &temporary).map_err(sync_error)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o700)).map_err(sync_error)?;
    fs::rename(temporary, &executable).map_err(sync_error)?;
    let mut args = vec![
        text(&executable)?.to_owned(),
        "--vault-path".into(),
        text(vault)?.to_owned(),
        "sync".into(),
    ];
    if let Some(unlock) = unlock {
        let unlock = unlock.canonicalize().map_err(sync_error)?;
        vault_sync::credentials::file::read_secret(&unlock).map_err(sync_error)?;
        args.extend(["--unlock-file".into(), text(&unlock)?.into()]);
    }
    args.push("watch".into());
    let home = dirs::home_dir().ok_or_else(|| sync_error("home directory unavailable"))?;
    if cfg!(target_os = "macos") {
        let path = home
            .join("Library/LaunchAgents")
            .join(format!("{label}.plist"));
        fs::create_dir_all(path.parent().unwrap()).map_err(sync_error)?;
        let arguments = args
            .iter()
            .map(|arg| format!("<string>{}</string>", xml(arg)))
            .collect::<String>();
        fs::write(&path, format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>{label}</string><key>ProgramArguments</key><array>{arguments}</array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>5</integer></dict></plist>")).map_err(sync_error)?;
        let domain = format!("gui/{}", uid()?);
        let _ = run("launchctl", &["bootout", &format!("{domain}/{label}")]);
        run("launchctl", &["bootstrap", &domain, text(&path)?])?;
    } else {
        let path = home
            .join(".config/systemd/user")
            .join(format!("{label}.service"));
        fs::create_dir_all(path.parent().unwrap()).map_err(sync_error)?;
        fs::write(&path, format!("[Unit]\nDescription=Loofah Staging Sync\n\n[Service]\nType=simple\nExecStart={}\nRestart=always\nRestartSec=5\nUMask=0077\n\n[Install]\nWantedBy=default.target\n", args.iter().map(|arg| unit(arg)).collect::<Vec<_>>().join(" "))).map_err(sync_error)?;
        run("systemctl", &["--user", "daemon-reload"])?;
        run(
            "systemctl",
            &["--user", "enable", "--now", &format!("{label}.service")],
        )?;
        eprintln!(
            "For operation after logout, your administrator may enable user lingering. Loofah does not change it."
        );
    }
    Ok(())
}

pub fn stop(directory: &Path) -> Result<()> {
    let label = label(directory)?;
    let home = dirs::home_dir().ok_or_else(|| sync_error("home directory unavailable"))?;
    if cfg!(target_os = "macos") {
        let path = home
            .join("Library/LaunchAgents")
            .join(format!("{label}.plist"));
        if path.exists() {
            run(
                "launchctl",
                &["bootout", &format!("gui/{}/{label}", uid()?)],
            )?;
            fs::remove_file(path).map_err(sync_error)?;
        }
    } else {
        let path = home
            .join(".config/systemd/user")
            .join(format!("{label}.service"));
        if path.exists() {
            run(
                "systemctl",
                &["--user", "disable", "--now", &format!("{label}.service")],
            )?;
            fs::remove_file(path).map_err(sync_error)?;
            run("systemctl", &["--user", "daemon-reload"])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_arguments_preserve_special_characters_without_expansion() {
        assert_eq!(
            unit("/vault/$HOME/100%/a\"b\n"),
            "\"/vault/$$HOME/100%%/a\\\"b\\n\""
        );
        assert_eq!(xml("<&\"'>"), "&lt;&amp;&quot;&apos;&gt;");
    }
}

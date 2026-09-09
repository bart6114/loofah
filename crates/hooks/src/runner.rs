use std::ffi::OsString;
use std::time::Duration;

use crate::config::HooksConfig;
use crate::event::HookEvent;

// Windows PowerShell's first launch also initializes the CLR; allow that startup
// without shortening the time available to an otherwise fast hook.
#[cfg(target_os = "windows")]
const HOOK_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(not(target_os = "windows"))]
const HOOK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct HookResult {
    pub command: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub async fn run_hooks_for_event(config: &HooksConfig, event: HookEvent) -> Vec<HookResult> {
    let condition_key = event.condition_key();
    let cli_args = event.cli_args();

    let Some(hooks) = config.on.get(condition_key) else {
        return vec![];
    };

    let futures: Vec<_> = hooks
        .iter()
        .map(|hook_def| {
            let command = hook_def.command.clone();
            let args = cli_args.clone();
            async move { execute_hook(&command, &args).await }
        })
        .collect();

    futures_util::future::join_all(futures).await
}

async fn read_output(mut reader: impl tokio::io::AsyncRead + Unpin) -> std::io::Result<String> {
    use tokio::io::AsyncReadExt;
    const OUTPUT_LIMIT: usize = 1024 * 1024;
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let keep = count.min(OUTPUT_LIMIT.saturating_sub(output.len()));
        output.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
    let mut output = String::from_utf8_lossy(&output).to_string();
    if truncated {
        output.push_str("\n[hook output truncated at 1 MiB]");
    }
    Ok(output)
}

#[cfg(not(target_os = "windows"))]
fn command_parts(command: &str) -> Result<Vec<String>, String> {
    shell_words::split(command).map_err(|error| error.to_string())
}

async fn execute_hook(command: &str, args: &[OsString]) -> HookResult {
    execute_hook_with_timeout(command, args, HOOK_TIMEOUT).await
}

async fn execute_hook_with_timeout(
    command: &str,
    args: &[OsString],
    timeout: Duration,
) -> HookResult {
    let fail = |error: String| HookResult {
        command: command.to_string(),
        success: false,
        exit_code: None,
        stdout: String::new(),
        stderr: error,
    };
    if command.trim().is_empty() {
        return fail("empty command".into());
    }
    let parts = match command_parts(command.trim()) {
        Ok(parts) if !parts.is_empty() => parts,
        Ok(_) => return fail("empty command".into()),
        Err(error) => return fail(format!("invalid hook command: {error}")),
    };
    // Expand individual arguments so an environment variable containing spaces
    // cannot change the executable or inject additional arguments.
    let parts: Vec<String> = parts
        .iter()
        .map(|part| {
            let expanded = shellexpand::full(part)
                .map(|s| s.into_owned())
                .unwrap_or_else(|_| part.clone());
            #[cfg(target_os = "windows")]
            let expanded = windows::expand_environment(&expanded);
            expanded
        })
        .collect();

    #[cfg(target_os = "windows")]
    let mut cmd = windows::command(&parts[0]);
    #[cfg(not(target_os = "windows"))]
    let mut cmd = tokio::process::Command::new(&parts[0]);
    cmd.args(&parts[1..]).args(args);
    let mut child = match cmd
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return fail(format!("failed to spawn command: {error}")),
    };
    #[cfg(target_os = "windows")]
    let _job = match windows::ProcessJob::new(&child) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill().await;
            return fail(format!("failed to supervise hook: {error}"));
        }
    };
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let completion =
        async { tokio::try_join!(child.wait(), read_output(stdout), read_output(stderr)) };
    match tokio::time::timeout(timeout, completion).await {
        Ok(Ok((status, stdout, stderr))) => HookResult {
            command: command.to_string(),
            success: status.success(),
            exit_code: status.code(),
            stdout,
            stderr,
        },
        Ok(Err(error)) => {
            let _ = child.kill().await;
            fail(format!("failed to read hook result: {error}"))
        }
        Err(_) => {
            let _ = child.kill().await;
            fail(format!(
                "hook timed out after {} seconds",
                timeout.as_secs()
            ))
        }
    }
}

#[cfg(target_os = "windows")]
use windows::command_parts;
#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod windows;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_command() {
        let result = execute_hook("", &[]).await;
        assert!(!result.success);
        assert_eq!(result.stderr, "empty command");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn successful_command() {
        let result = execute_hook("echo hello", &[]).await;
        assert!(result.success);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout.trim(), "hello");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn failed_command() {
        let result = execute_hook("false", &[]).await;
        assert!(!result.success);
        assert_eq!(result.exit_code, Some(1));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn with_cli_args() {
        let args = vec![OsString::from("world")];
        let result = execute_hook("echo", &args).await;
        assert!(result.success);
        assert_eq!(result.stdout.trim(), "world");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn expands_home_env_var() {
        let home = std::env::var("HOME").unwrap();
        let result = execute_hook("echo $HOME", &[]).await;
        assert!(result.success);
        assert_eq!(result.stdout.trim(), home);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn expands_tilde_in_command_path() {
        let result = execute_hook("~/../../bin/echo tilde_works", &[]).await;
        assert!(result.success);
        assert_eq!(result.stdout.trim(), "tilde_works");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn drains_large_stdout_and_stderr_while_the_hook_runs() {
        let result = execute_hook(
            r#"sh -c 'head -c 1200000 /dev/zero; head -c 1200000 /dev/zero >&2'"#,
            &[],
        )
        .await;
        assert!(result.success, "{}", result.stderr);
        assert!(result.stdout.ends_with("[hook output truncated at 1 MiB]"));
        assert!(result.stderr.ends_with("[hook output truncated at 1 MiB]"));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn preserves_quoted_arguments() {
        let result = execute_hook(r#"printf '[%s]' 'meeting with spaces'"#, &[]).await;
        assert!(result.success);
        assert_eq!(result.stdout, "[meeting with spaces]");
    }

    #[tokio::test]
    #[cfg(target_os = "windows")]
    async fn powershell_script_receives_event_arguments_literally() {
        let directory = tempfile::Builder::new()
            .prefix("loofah hooks ")
            .tempdir()
            .unwrap();
        let script = directory.path().join("meeting hook.ps1");
        std::fs::write(&script, "param([string]$Value)\n[Console]::Write($Value)\n").unwrap();
        let value = "meeting & $HOME; 'quoted'";
        // This checks quoting, not cold CLR startup speed on a shared CI runner.
        let result = execute_hook_with_timeout(
            &format!("\"{}\"", script.display()),
            &[value.into()],
            Duration::from_secs(60),
        )
        .await;
        assert!(result.success, "{}", result.stderr);
        assert_eq!(result.stdout, value);
    }

    #[tokio::test]
    async fn nonexistent_command() {
        let result = execute_hook("nonexistent_command_12345", &[]).await;
        assert!(!result.success);
        assert!(result.stderr.contains("failed to spawn command"));
    }

    #[test]
    #[ignore = "subprocess fixture for hook timeout test"]
    fn slow_hook() {
        std::thread::sleep(Duration::from_secs(30));
    }

    #[tokio::test]
    async fn terminates_a_hook_that_exceeds_its_deadline() {
        let executable = std::env::current_exe().unwrap();
        let result = execute_hook_with_timeout(
            &format!("\"{}\"", executable.display()),
            &[
                "--exact".into(),
                "runner::tests::slow_hook".into(),
                "--ignored".into(),
            ],
            Duration::from_secs(1),
        )
        .await;
        assert!(!result.success);
        assert_eq!(result.stderr, "hook timed out after 1 seconds");
    }
}

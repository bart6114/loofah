use ::windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree},
        System::{
            Environment::ExpandEnvironmentStringsW,
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
            Threading::CREATE_NO_WINDOW,
        },
        UI::Shell::CommandLineToArgvW,
    },
    core::PCWSTR,
};

pub(super) fn command_parts(command: &str) -> Result<Vec<String>, String> {
    let wide: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
    let mut count = 0;
    unsafe {
        let pointer = CommandLineToArgvW(PCWSTR(wide.as_ptr()), &mut count);
        if pointer.is_null() {
            return Err(::windows::core::Error::from_thread().to_string());
        }
        let result = std::slice::from_raw_parts(pointer, count as usize)
            .iter()
            .map(|value| value.to_string().map_err(|e| e.to_string()))
            .collect();
        let _ = LocalFree(Some(HLOCAL(pointer.cast())));
        result
    }
}

pub(super) fn expand_environment(value: &str) -> String {
    let wide: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let size = ExpandEnvironmentStringsW(PCWSTR(wide.as_ptr()), None);
        if size == 0 {
            return value.to_string();
        }
        let mut output = vec![0; size as usize];
        let written = ExpandEnvironmentStringsW(PCWSTR(wide.as_ptr()), Some(&mut output));
        if written == 0 || written > size {
            return value.to_string();
        }
        String::from_utf16_lossy(&output[..written as usize - 1])
    }
}

pub(super) fn command(executable: &str) -> tokio::process::Command {
    let mut command = if std::path::Path::new(executable)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ps1"))
    {
        let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let powershell = std::path::PathBuf::from(system_root)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut command = tokio::process::Command::new(powershell);
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-File",
            executable,
        ]);
        command
    } else {
        tokio::process::Command::new(executable)
    };
    command.creation_flags(CREATE_NO_WINDOW.0);
    command
}

pub(super) struct ProcessJob(HANDLE);

// The owned kernel handle is only closed after the hook's asynchronous wait ends.
unsafe impl Send for ProcessJob {}

impl ProcessJob {
    pub(super) fn new(child: &tokio::process::Child) -> Result<Self, String> {
        let process = child
            .raw_handle()
            .ok_or("The hook exited before supervision started")?;
        unsafe {
            let job = Self(CreateJobObjectW(None, None).map_err(|e| e.to_string())?);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            )
            .map_err(|e| e.to_string())?;
            AssignProcessToJobObject(job.0, HANDLE(process)).map_err(|e| e.to_string())?;
            Ok(job)
        }
    }
}

impl Drop for ProcessJob {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_windows_paths_without_losing_backslashes() {
        assert_eq!(
            command_parts(r#""C:\Users\Bart Smeets\hook.ps1" "meeting title" "C:\vault\\""#)
                .unwrap(),
            [
                r"C:\Users\Bart Smeets\hook.ps1",
                "meeting title",
                "C:\\vault\\"
            ]
        );
    }
}

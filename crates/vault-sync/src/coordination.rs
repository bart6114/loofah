use std::path::Path;

/// Runs the whole read/compare/write transaction while iCloud file presenters
/// hold the requested directory. Call on a blocking worker, never the UI thread.
pub fn coordinate<T>(root: &Path, write: bool, operation: impl FnOnce() -> T) -> crate::Result<T> {
    #[cfg(target_vendor = "apple")]
    {
        use std::ffi::{CString, c_char, c_void};
        use std::os::unix::ffi::OsStrExt;
        unsafe extern "C" {
            fn loofah_coordinate(
                path: *const c_char,
                writing: bool,
                context: *mut c_void,
                accessor: unsafe extern "C" fn(*mut c_void),
                error: *mut c_char,
                capacity: usize,
            ) -> bool;
        }
        struct Context<F, T> {
            operation: Option<F>,
            result: Option<std::thread::Result<T>>,
        }
        unsafe extern "C" fn invoke<F: FnOnce() -> T, T>(raw: *mut c_void) {
            let context = unsafe { &mut *(raw as *mut Context<F, T>) };
            context.result = Some(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                context.operation.take().unwrap(),
            )));
        }
        fn run<F: FnOnce() -> T, T>(root: &Path, write: bool, operation: F) -> crate::Result<T> {
            let path = CString::new(root.as_os_str().as_bytes())
                .map_err(|_| crate::Error::UnsafePath(root.to_path_buf()))?;
            let mut context = Context {
                operation: Some(operation),
                result: None,
            };
            let mut error = [0 as c_char; 1024];
            let ok = unsafe {
                loofah_coordinate(
                    path.as_ptr(),
                    write,
                    &mut context as *mut _ as *mut c_void,
                    invoke::<F, T>,
                    error.as_mut_ptr(),
                    error.len(),
                )
            };
            if !ok {
                let message = unsafe { std::ffi::CStr::from_ptr(error.as_ptr()) }
                    .to_string_lossy()
                    .into_owned();
                return Err(crate::Error::Coordination(message));
            }
            match context
                .result
                .expect("native coordinator did not invoke accessor")
            {
                Ok(result) => Ok(result),
                Err(panic) => std::panic::resume_unwind(panic),
            }
        }
        run(root, write, operation)
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        let _ = (root, write);
        Ok(operation())
    }
}

pub fn unresolved_versions(path: &Path) -> crate::Result<Vec<(std::path::PathBuf, String)>> {
    #[cfg(target_vendor = "apple")]
    {
        use std::ffi::{CStr, CString, c_char, c_void};
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        unsafe extern "C" {
            fn loofah_versions(
                path: *const c_char,
                context: *mut c_void,
                version: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char),
                error: *mut c_char,
                capacity: usize,
            ) -> bool;
        }
        unsafe extern "C" fn collect(
            context: *mut c_void,
            path: *const c_char,
            token: *const c_char,
        ) {
            let paths = unsafe { &mut *(context as *mut Vec<(std::path::PathBuf, String)>) };
            if paths.len() < 16 {
                let bytes = unsafe { CStr::from_ptr(path) }.to_bytes().to_vec();
                let token = unsafe { CStr::from_ptr(token) }
                    .to_string_lossy()
                    .into_owned();
                paths.push((std::ffi::OsString::from_vec(bytes).into(), token));
            }
        }
        let path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| crate::Error::UnsafePath(path.into()))?;
        let mut paths = Vec::<(std::path::PathBuf, String)>::new();
        let mut error = [0 as c_char; 1024];
        let ok = unsafe {
            loofah_versions(
                path.as_ptr(),
                &mut paths as *mut _ as *mut c_void,
                collect,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if !ok {
            return Err(crate::Error::Coordination(
                unsafe { CStr::from_ptr(error.as_ptr()) }
                    .to_string_lossy()
                    .into_owned(),
            ));
        }
        Ok(paths)
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        let _ = path;
        Ok(Vec::new())
    }
}

pub fn resolve_version(path: &Path, version: &str) -> crate::Result<()> {
    #[cfg(target_vendor = "apple")]
    {
        use std::ffi::{CString, c_char};
        use std::os::unix::ffi::OsStrExt;
        unsafe extern "C" {
            fn loofah_resolve_version(path: *const c_char, version: *const c_char);
        }
        let path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| crate::Error::UnsafePath(path.into()))?;
        let version = CString::new(version.as_bytes())
            .map_err(|_| crate::Error::UnsafePath(version.into()))?;
        unsafe {
            loofah_resolve_version(path.as_ptr(), version.as_ptr());
        }
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        let _ = (path, version);
    }
    Ok(())
}

/// Starts provider hydration without holding a directory write claim while the
/// provider downloads. The next foreground pass retries instead of busy-waiting.
pub fn ensure_available(path: &Path) -> crate::Result<()> {
    #[cfg(target_vendor = "apple")]
    {
        use std::ffi::{CStr, CString, c_char};
        use std::os::unix::ffi::OsStrExt;
        unsafe extern "C" {
            fn loofah_materialize(path: *const c_char, error: *mut c_char, capacity: usize) -> i32;
        }
        let path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| crate::Error::UnsafePath(path.into()))?;
        let mut error = [0 as c_char; 1024];
        match unsafe { loofah_materialize(path.as_ptr(), error.as_mut_ptr(), error.len()) } {
            0 => Ok(()),
            1 => Err(crate::Error::Coordination(
                "Downloading from iCloud; retry when the file is available".into(),
            )),
            _ => Err(crate::Error::Coordination(
                unsafe { CStr::from_ptr(error.as_ptr()) }
                    .to_string_lossy()
                    .into_owned(),
            )),
        }
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        let _ = path;
        Ok(())
    }
}

//! This crate provides low-level bindings to `librclone`.
//!
//! See the `librclone` crate for details.

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

#[cfg(not(windows))]
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

#[cfg(not(windows))]
pub unsafe fn try_RcloneInitialize() -> Result<(), String> {
    unsafe { RcloneInitialize() };
    Ok(())
}

#[cfg(not(windows))]
pub unsafe fn try_RcloneFinalize() -> Result<(), String> {
    unsafe { RcloneFinalize() };
    Ok(())
}

#[cfg(not(windows))]
pub unsafe fn try_RcloneRPC(
    method: *mut std::os::raw::c_char,
    input: *mut std::os::raw::c_char,
) -> Result<RcloneRPCResult, String> {
    Ok(unsafe { RcloneRPC(method, input) })
}

#[cfg(not(windows))]
pub unsafe fn try_RcloneFreeString(value: *mut std::os::raw::c_char) -> Result<(), String> {
    unsafe { RcloneFreeString(value) };
    Ok(())
}

#[cfg(windows)]
mod windows {
    use std::{
        env,
        os::raw::{c_char, c_int},
        path::PathBuf,
        sync::OnceLock,
    };

    use libloading::Library;

    const DLL_NAME: &str = "librclone.dll";

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    pub struct RcloneRPCResult {
        pub Output: *mut c_char,
        pub Status: c_int,
    }

    type RcloneInitializeFn = unsafe extern "C" fn();
    type RcloneFinalizeFn = unsafe extern "C" fn();
    type RcloneRPCFn = unsafe extern "C" fn(*mut c_char, *mut c_char) -> RcloneRPCResult;
    type RcloneFreeStringFn = unsafe extern "C" fn(*mut c_char);

    static LIBRCLONE_LIBRARY: OnceLock<Result<Library, String>> = OnceLock::new();

    pub unsafe fn RcloneInitialize() {
        unsafe { try_RcloneInitialize() }.expect("load and call RcloneInitialize");
    }

    pub unsafe fn RcloneFinalize() {
        unsafe { try_RcloneFinalize() }.expect("load and call RcloneFinalize");
    }

    pub unsafe fn RcloneRPC(method: *mut c_char, input: *mut c_char) -> RcloneRPCResult {
        unsafe { try_RcloneRPC(method, input) }.expect("load and call RcloneRPC")
    }

    pub unsafe fn RcloneFreeString(value: *mut c_char) {
        unsafe { try_RcloneFreeString(value) }.expect("load and call RcloneFreeString");
    }

    pub unsafe fn try_RcloneInitialize() -> Result<(), String> {
        let library = librclone_library()?;
        let function = unsafe { library.get::<RcloneInitializeFn>(b"RcloneInitialize") }
            .map_err(|error| format!("failed to load RcloneInitialize from {DLL_NAME}: {error}"))?;
        unsafe { function() };
        Ok(())
    }

    pub unsafe fn try_RcloneFinalize() -> Result<(), String> {
        let library = librclone_library()?;
        let function = unsafe { library.get::<RcloneFinalizeFn>(b"RcloneFinalize") }
            .map_err(|error| format!("failed to load RcloneFinalize from {DLL_NAME}: {error}"))?;
        unsafe { function() };
        Ok(())
    }

    pub unsafe fn try_RcloneRPC(
        method: *mut c_char,
        input: *mut c_char,
    ) -> Result<RcloneRPCResult, String> {
        let library = librclone_library()?;
        let function = unsafe { library.get::<RcloneRPCFn>(b"RcloneRPC") }
            .map_err(|error| format!("failed to load RcloneRPC from {DLL_NAME}: {error}"))?;
        Ok(unsafe { function(method, input) })
    }

    pub unsafe fn try_RcloneFreeString(value: *mut c_char) -> Result<(), String> {
        let library = librclone_library()?;
        let function = unsafe { library.get::<RcloneFreeStringFn>(b"RcloneFreeString") }
            .map_err(|error| format!("failed to load RcloneFreeString from {DLL_NAME}: {error}"))?;
        unsafe { function(value) };
        Ok(())
    }

    fn librclone_library() -> Result<&'static Library, String> {
        match LIBRCLONE_LIBRARY.get_or_init(load_librclone_library) {
            Ok(library) => Ok(library),
            Err(error) => Err(error.clone()),
        }
    }

    fn load_librclone_library() -> Result<Library, String> {
        let mut attempts = Vec::new();

        for candidate in librclone_dll_candidates() {
            let label = candidate.display().to_string();
            match unsafe { Library::new(&candidate) } {
                Ok(library) => return Ok(library),
                Err(error) => attempts.push(format!("{label}: {error}")),
            }
        }

        Err(format!(
            "unable to load {DLL_NAME}; searched {}",
            attempts.join("; ")
        ))
    }

    fn librclone_dll_candidates() -> Vec<PathBuf> {
        let mut candidates = Vec::new();

        if let Some(configured) = env::var_os("LIBRCLONE_DLL_PATH") {
            let configured = PathBuf::from(configured);
            candidates.push(if configured.is_dir() {
                configured.join(DLL_NAME)
            } else {
                configured
            });
        }

        if let Ok(current_exe) = env::current_exe() {
            if let Some(directory) = current_exe.parent() {
                candidates.push(directory.join(DLL_NAME));
            }
        }

        if let Some(out_dir) = option_env!("LIBRCLONE_BUILD_OUT_DIR") {
            candidates.push(PathBuf::from(out_dir).join(DLL_NAME));
        }

        candidates.push(PathBuf::from(DLL_NAME));
        candidates
    }
}

#[cfg(windows)]
pub use windows::{
    RcloneFinalize, RcloneFreeString, RcloneInitialize, RcloneRPC, RcloneRPCResult,
    try_RcloneFinalize, try_RcloneFreeString, try_RcloneInitialize, try_RcloneRPC,
};

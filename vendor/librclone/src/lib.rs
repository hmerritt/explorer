#![doc = include_str!("../README.md")]

use std::{
    error::Error,
    ffi::{CStr, CString, NulError},
    fmt,
    os::raw::c_char,
    str::Utf8Error,
};

/// Errors returned by the fallible librclone wrapper APIs.
#[derive(Debug)]
pub enum LibrcloneError {
    /// The low-level library could not be loaded or called.
    Library(String),
    /// A method or input string contained an interior NUL byte and cannot be passed to C.
    InteriorNul(NulError),
    /// librclone returned a null output pointer.
    NullOutput { status: i32 },
    /// librclone returned output that was not valid UTF-8.
    InvalidUtf8(Utf8Error),
    /// librclone completed the RPC call with a non-success status.
    Rpc { status: i32, output: String },
}

impl fmt::Display for LibrcloneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Library(error) => formatter.write_str(error),
            Self::InteriorNul(error) => {
                write!(formatter, "librclone input contains NUL byte: {error}")
            }
            Self::NullOutput { status } => {
                write!(
                    formatter,
                    "librclone returned null output with status {status}"
                )
            }
            Self::InvalidUtf8(error) => {
                write!(formatter, "librclone returned invalid UTF-8: {error}")
            }
            Self::Rpc { status, output } => {
                write!(
                    formatter,
                    "librclone RPC failed with status {status}: {output}"
                )
            }
        }
    }
}

impl Error for LibrcloneError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InteriorNul(error) => Some(error),
            Self::InvalidUtf8(error) => Some(error),
            Self::Library(_) | Self::NullOutput { .. } | Self::Rpc { .. } => None,
        }
    }
}

impl From<NulError> for LibrcloneError {
    fn from(error: NulError) -> Self {
        Self::InteriorNul(error)
    }
}

impl From<Utf8Error> for LibrcloneError {
    fn from(error: Utf8Error) -> Self {
        Self::InvalidUtf8(error)
    }
}

/// Initializes rclone as a library.
pub fn initialize() {
    try_initialize().expect("initialize librclone");
}

/// Fallible variant of [`initialize`].
pub fn try_initialize() -> Result<(), LibrcloneError> {
    unsafe { librclone_sys::try_RcloneInitialize() }.map_err(LibrcloneError::Library)
}

/// Finalizes rclone as a library. Currently just calls the Go GC; don't stress if you never call it. :-)
pub fn finalize() {
    try_finalize().expect("finalize librclone");
}

/// Fallible variant of [`finalize`].
pub fn try_finalize() -> Result<(), LibrcloneError> {
    unsafe { librclone_sys::try_RcloneFinalize() }.map_err(LibrcloneError::Library)
}

/// Does a single librclone RPC call.
/// - `method`: e.g. `operations/list`, from <https://rclone.org/rc/#supported-commands>
/// - `input`: a serialized JSON object.
/// - Return value (`Ok` or `Err`) is a serialized JSON String.
pub fn rpc<S1: Into<String>, S2: Into<String>>(method: S1, input: S2) -> Result<String, String> {
    match try_rpc(method, input) {
        Ok(output) => Ok(output),
        Err(LibrcloneError::Rpc { output, .. }) => Err(output),
        Err(error) => panic!("librclone RPC failed: {error}"),
    }
}

/// Fallible variant of [`rpc`].
pub fn try_rpc<S1: Into<String>, S2: Into<String>>(
    method: S1,
    input: S2,
) -> Result<String, LibrcloneError> {
    let method = CString::new(method.into())?;
    let input = CString::new(input.into())?;

    let result = unsafe {
        librclone_sys::try_RcloneRPC(
            method.as_ptr() as *mut c_char,
            input.as_ptr() as *mut c_char,
        )
    }
    .map_err(LibrcloneError::Library)?;

    if result.Output.is_null() {
        return Err(LibrcloneError::NullOutput {
            status: result.Status,
        });
    }

    let output_result = unsafe { CStr::from_ptr(result.Output) }
        .to_str()
        .map(str::to_owned)
        .map_err(LibrcloneError::from);
    let free_result = unsafe { librclone_sys::try_RcloneFreeString(result.Output) }
        .map_err(LibrcloneError::Library);

    let output = output_result?;
    free_result?;

    match result.Status {
        200 => Ok(output),
        status => Err(LibrcloneError::Rpc { status, output }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_test_rpc_and_version() {
        try_initialize().expect("initialize librclone");

        assert_eq!(try_rpc("rc/noop", "{}").expect("rc/noop"), "{}\n");

        let version = try_rpc("core/version", "{}").expect("core/version");
        assert!(
            version.contains("\"version\": \"v1.74.3\"")
                || version.contains("\"version\":\"v1.74.3\""),
            "unexpected core/version response: {version}"
        );

        let error = try_rpc("rc/error", "{}").expect_err("rc/error fails");
        assert!(matches!(error, LibrcloneError::Rpc { status: 500, .. }));

        try_finalize().expect("finalize librclone");
    }
}

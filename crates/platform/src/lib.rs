//! Thin wrappers over the Win32, DWM and shell APIs Nook uses.
//!
//! Every call into Windows goes through a function here that returns a
//! [`Result`] naming the API that failed, so the rest of the app never sees
//! raw `BOOL`s, `HRESULT`s or handles.

pub mod audio;
pub mod backdrop;
pub mod capture;
pub mod clipboard;
pub mod embed;
pub mod files;
pub mod fontmatch;
pub mod fontnet;
pub mod fullscreen;
pub mod hittest;
pub mod ink;
pub mod ipc;
pub mod material;
pub mod media;
pub mod monitor;
pub mod ocr;
pub mod perf;
pub mod power;
pub mod process;
pub mod raster;
pub mod registry;
pub mod shell;
pub mod single_instance;
pub mod snip;
pub mod startup;
pub mod system;
pub mod url_scheme;
pub mod wallpaper;
pub mod window;

pub use window::Hwnd;

/// A failed Windows API call.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("{call} failed: {source}")]
    Win32 {
        call: &'static str,
        #[source]
        source: windows::core::Error,
    },
    #[error("{call} failed with Win32 error {code}")]
    Code { call: &'static str, code: u32 },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, PlatformError>;

/// Attaches the API name to a `windows::core::Result`.
pub(crate) trait Win32Context<T> {
    fn api(self, call: &'static str) -> Result<T>;
}

impl<T> Win32Context<T> for windows::core::Result<T> {
    fn api(self, call: &'static str) -> Result<T> {
        self.map_err(|source| PlatformError::Win32 { call, source })
    }
}

/// Converts a `WIN32_ERROR` return code into a `Result`.
pub(crate) fn check(
    call: &'static str,
    err: windows::Win32::Foundation::WIN32_ERROR,
) -> Result<()> {
    if err.is_ok() {
        Ok(())
    } else {
        Err(PlatformError::Code { call, code: err.0 })
    }
}

/// Null-terminated UTF-16 for `PCWSTR` parameters.
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

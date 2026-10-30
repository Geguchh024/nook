//! Last-chance log line for native crashes: access violations inside a GPU
//! driver or a system DLL never reach the Rust panic hook, and the process
//! just disappears. The unhandled-exception filter appends the exception
//! code and the faulting module to the log before Windows ends the process.

use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::Diagnostics::Debug::{EXCEPTION_POINTERS, SetUnhandledExceptionFilter};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW,
};
use windows::core::PCWSTR;

static LOG: OnceLock<PathBuf> = OnceLock::new();

/// Appends native crashes to `log`. Call once, early.
pub fn install(log: PathBuf) {
    if LOG.set(log).is_ok() {
        // SAFETY: the filter is a plain function that lives forever.
        unsafe {
            SetUnhandledExceptionFilter(Some(filter));
        }
    }
}

/// `nvoglv64.dll+0x1a2b3c` for an address inside a loaded module.
fn module_of(addr: usize) -> String {
    let mut module = HMODULE::default();
    // SAFETY: FROM_ADDRESS reads the "name" as an address; the refcount is
    // left alone so nothing needs freeing.
    let found = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(addr as *const u16),
            &mut module,
        )
    };
    if found.is_err() {
        return format!("{addr:#x} (no module)");
    }
    let mut buf = [0u16; 260];
    // SAFETY: sized buffer, module handle from above.
    let n = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    let path = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
    let name = path.rsplit('\\').next().unwrap_or(&path).to_owned();
    format!("{name}+{:#x}", addr.wrapping_sub(module.0 as usize))
}

unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
    // SAFETY: Windows passes valid exception pointers to the filter.
    let (code, addr) = unsafe {
        match info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()) {
            Some(rec) => (rec.ExceptionCode.0 as u32, rec.ExceptionAddress as usize),
            None => (0, 0),
        }
    };
    if let Some(path) = LOG.get()
        && let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(path)
    {
        let _ = writeln!(
            f,
            "{} FATAL native crash: exception {code:#010X} ({}) at {}",
            crate::system::file_timestamp(),
            describe(code),
            module_of(addr),
        );
    }
    // EXCEPTION_CONTINUE_SEARCH: let Windows Error Reporting carry on.
    0
}

fn describe(code: u32) -> &'static str {
    match code {
        0xC000_0005 => "access violation",
        0xC000_001D => "illegal instruction",
        0xC000_00FD => "stack overflow",
        0xC000_0094 => "integer divide by zero",
        0xC000_0409 => "stack buffer overrun / fail-fast",
        0xC000_0374 => "heap corruption",
        0x8000_0003 => "breakpoint",
        _ => "unknown",
    }
}

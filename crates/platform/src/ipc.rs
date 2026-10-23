//! Messages from a second launch to the running Nook, over a named pipe.
//!
//! The browser starts `nook.exe "nook://install?..."` for every install
//! link. Only one Nook runs per session, so that second process hands the
//! link to the first one here and exits. The pipe takes inbound bytes only,
//! rejects remote clients, and keeps the default DACL (only the creating
//! user, administrators and SYSTEM may write).

use crate::{PlatformError, Result, wide};
use std::io::{Read, Write};
use std::os::windows::io::FromRawHandle;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::core::PCWSTR;

/// Longest message accepted; anything past it is dropped.
const MAX_MESSAGE: u64 = 16 * 1024;

/// `\\.\pipe\<name>.<user>`: one pipe per user, like the instance mutex.
fn pipe_path(name: &str) -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let user: String = user
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    format!(r"\\.\pipe\{name}.{user}")
}

/// Listens on a thread for the life of the process and calls `on_message`
/// with each message received.
pub fn serve(name: &str, on_message: impl Fn(String) + Send + 'static) -> Result<()> {
    let path = wide(&pipe_path(name));
    std::thread::Builder::new()
        .name("nook-ipc".into())
        .spawn(move || {
            let mut first = true;
            loop {
                let mut mode = PIPE_ACCESS_INBOUND;
                if first {
                    // Fails if someone else already owns the name.
                    mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
                }
                // SAFETY: valid name; the handle is owned by the File below.
                let handle = unsafe {
                    CreateNamedPipeW(
                        PCWSTR(path.as_ptr()),
                        mode,
                        PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                        PIPE_UNLIMITED_INSTANCES,
                        0,
                        MAX_MESSAGE as u32,
                        0,
                        None,
                    )
                };
                if handle == INVALID_HANDLE_VALUE || handle.is_invalid() {
                    tracing::warn!(
                        "ipc: CreateNamedPipeW failed: {}",
                        windows::core::Error::from_thread()
                    );
                    return;
                }
                first = false;
                // SAFETY: a fresh pipe handle; blocks until a client connects.
                let connected = match unsafe { ConnectNamedPipe(handle, None) } {
                    Ok(()) => true,
                    Err(e) => e.code() == ERROR_PIPE_CONNECTED.to_hresult(),
                };
                // SAFETY: we own the handle; File closes it on drop.
                let mut pipe = unsafe { std::fs::File::from_raw_handle(handle.0) };
                if !connected {
                    continue;
                }
                let mut text = String::new();
                match (&mut pipe).take(MAX_MESSAGE).read_to_string(&mut text) {
                    Ok(_) if !text.trim().is_empty() => on_message(text.trim().to_owned()),
                    Ok(_) => {}
                    Err(e) => tracing::warn!("ipc read: {e}"),
                }
            }
        })
        .map_err(|e| PlatformError::Other(format!("spawning ipc thread: {e}")))?;
    Ok(())
}

/// Sends `message` to the process serving `name`. Retries for a moment, in
/// case that process is between two connections or still starting.
pub fn send(name: &str, message: &str) -> Result<()> {
    let path = pipe_path(name);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match std::fs::OpenOptions::new().write(true).open(&path) {
            Ok(mut pipe) => {
                return pipe
                    .write_all(message.as_bytes())
                    .map_err(|e| PlatformError::Other(format!("ipc write: {e}")));
            }
            Err(e) if Instant::now() >= deadline => {
                return Err(PlatformError::Other(format!("ipc open {path}: {e}")));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivers_messages_in_order() {
        let name = format!("Nook.Test.{}", std::process::id());
        let (tx, rx) = std::sync::mpsc::channel();
        serve(&name, move |m| {
            let _ = tx.send(m);
        })
        .unwrap();
        send(&name, "nook://install?id=one").unwrap();
        send(&name, "nook://install?id=two").unwrap();
        let got: Vec<String> = (0..2)
            .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect();
        assert_eq!(got, ["nook://install?id=one", "nook://install?id=two"]);
    }
}

//! Keep-awake via `SetThreadExecutionState`.
//!
//! The execution state is per-thread and is released automatically by
//! Windows when the thread (or process) ends, so even a crash cannot leave
//! the machine stuck awake. Call these from one long-lived thread.

use crate::{PlatformError, Result};
use windows::Win32::System::Power::{
    ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
};

pub fn keep_awake(display: bool) -> Result<()> {
    let mut flags = ES_CONTINUOUS | ES_SYSTEM_REQUIRED;
    if display {
        flags |= ES_DISPLAY_REQUIRED;
    }
    // SAFETY: plain flag call; returns 0 on failure.
    if unsafe { SetThreadExecutionState(flags) }.0 == 0 {
        return Err(PlatformError::Other(
            "SetThreadExecutionState failed".into(),
        ));
    }
    Ok(())
}

/// Back to normal power management.
pub fn release() -> Result<()> {
    // SAFETY: plain flag call.
    if unsafe { SetThreadExecutionState(ES_CONTINUOUS) }.0 == 0 {
        return Err(PlatformError::Other(
            "SetThreadExecutionState(reset) failed".into(),
        ));
    }
    Ok(())
}

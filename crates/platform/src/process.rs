//! Process-level housekeeping.

use crate::{PlatformError, Result};
use windows::Win32::System::ProcessStatus::K32EmptyWorkingSet;
use windows::Win32::System::Threading::GetCurrentProcess;

/// Releases this process's working set back to the OS standby list.
///
/// Used once the shell has sat collapsed for a while: the OpenGL driver
/// keeps tens of MB resident that an idle notch never touches. Pages fault
/// back in (from RAM, not disk) on the next expand.
pub fn trim_working_set() -> Result<()> {
    // SAFETY: pseudo-handle of the current process.
    if unsafe { K32EmptyWorkingSet(GetCurrentProcess()) }.as_bool() {
        Ok(())
    } else {
        Err(PlatformError::Other("K32EmptyWorkingSet failed".into()))
    }
}

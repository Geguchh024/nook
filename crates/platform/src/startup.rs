//! Launch at sign-in via the per-user `Run` key.

use crate::registry::{self, Hive};
use crate::{PlatformError, Result};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "Nook";

pub fn is_enabled() -> bool {
    registry::read_string(Hive::CurrentUser, RUN_KEY, VALUE).is_some()
}

pub fn set_enabled(enabled: bool) -> Result<()> {
    if enabled {
        let exe = std::env::current_exe()
            .map_err(|e| PlatformError::Other(format!("current_exe: {e}")))?;
        let cmd = format!("\"{}\"", exe.display());
        registry::write_string(Hive::CurrentUser, RUN_KEY, VALUE, &cmd)
    } else {
        registry::delete_value(Hive::CurrentUser, RUN_KEY, VALUE)
    }
}

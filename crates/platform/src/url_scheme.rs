//! Registers a `name://` link type for the current user, so a browser can
//! start Nook with a link (`HKCU\Software\Classes\<name>`).

use crate::registry::{self, Hive};
use crate::{PlatformError, Result};

/// Points `scheme://` links at this executable. Writes only what differs,
/// so calling it at every start costs a few registry reads.
pub fn register(scheme: &str, description: &str) -> Result<()> {
    let exe =
        std::env::current_exe().map_err(|e| PlatformError::Other(format!("current_exe: {e}")))?;
    let exe = exe.display().to_string();
    let key = format!(r"Software\Classes\{scheme}");
    let command = format!("\"{exe}\" \"%1\"");
    let values = [
        (key.clone(), "", format!("URL:{description}")),
        (key.clone(), "URL Protocol", String::new()),
        (format!(r"{key}\DefaultIcon"), "", format!("\"{exe}\",0")),
        (format!(r"{key}\shell\open\command"), "", command),
    ];
    for (subkey, name, data) in values {
        if registry::read_string(Hive::CurrentUser, &subkey, name).as_deref() != Some(&data) {
            registry::write_string(Hive::CurrentUser, &subkey, name, &data)?;
        }
    }
    Ok(())
}

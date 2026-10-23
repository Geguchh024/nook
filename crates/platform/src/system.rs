//! System preferences: theme, animations, transparency, build number,
//! accent color and wallpaper path.

use crate::registry::{self, Hive};
use crate::{Result, Win32Context};
use windows::Win32::Graphics::Dwm::DwmGetColorizationColor;
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::UI::WindowsAndMessaging::{
    MB_ICONASTERISK, SPI_GETCLIENTAREAANIMATION, SPI_GETDESKWALLPAPER,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};
use windows::core::BOOL;

const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

/// Windows "app mode" is light.
pub fn apps_use_light_theme() -> bool {
    registry::read_dword(Hive::CurrentUser, PERSONALIZE, "AppsUseLightTheme").unwrap_or(0) != 0
}

/// "Transparency effects" toggle in Settings > Personalization > Colors.
pub fn transparency_enabled() -> bool {
    registry::read_dword(Hive::CurrentUser, PERSONALIZE, "EnableTransparency").unwrap_or(1) != 0
}

/// "Animation effects" (Settings > Accessibility > Visual effects).
pub fn animations_enabled() -> bool {
    let mut on = BOOL(1);
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes a BOOL.
    let res = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut on as *mut BOOL as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    res.is_err() || on.as_bool()
}

/// Windows build number, e.g. 22631. 0 if unknown.
pub fn windows_build() -> u32 {
    registry::read_string(
        Hive::LocalMachine,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "CurrentBuildNumber",
    )
    .and_then(|s| s.trim().parse().ok())
    .unwrap_or(0)
}

/// The DWM colorization (accent) color as RGB.
pub fn system_accent() -> Result<[u8; 3]> {
    let mut argb = 0u32;
    let mut opaque = BOOL(0);
    // SAFETY: valid out pointers.
    unsafe { DwmGetColorizationColor(&mut argb, &mut opaque) }.api("DwmGetColorizationColor")?;
    Ok([(argb >> 16) as u8, (argb >> 8) as u8, argb as u8])
}

/// Path of the current desktop wallpaper image, if any.
pub fn wallpaper_path() -> Option<std::path::PathBuf> {
    let mut buf = [0u16; 520];
    // SAFETY: buffer length is passed in characters.
    let res = unsafe {
        SystemParametersInfoW(
            SPI_GETDESKWALLPAPER,
            buf.len() as u32,
            Some(buf.as_mut_ptr() as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    res.ok()?;
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    if len == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len]).into())
}

/// Local time and date, formatted with the user's Windows locale settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clock {
    /// e.g. "14:32" or "2:32 PM".
    pub time: String,
    /// Short weekday and day of month, e.g. "Mon 19".
    pub date: String,
    /// Seconds until the minute changes (for scheduling the next repaint).
    pub secs_to_next_minute: u32,
}

/// Local date and time for file names: `2026-10-20 04-31-12`.
pub fn file_timestamp() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: plain query returning a value.
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}-{:02}-{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

/// Local date and time, split into fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    /// 1..=12.
    pub month: u32,
    /// 1..=31.
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

pub fn local_time() -> LocalTime {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: plain query returning a value.
    let t = unsafe { GetLocalTime() };
    LocalTime {
        year: i32::from(t.wYear),
        month: u32::from(t.wMonth),
        day: u32::from(t.wDay),
        hour: u32::from(t.wHour),
        minute: u32::from(t.wMinute),
        second: u32::from(t.wSecond),
    }
}

/// First day of the week in the user's locale: 0 = Monday ... 6 = Sunday.
pub fn first_day_of_week() -> u32 {
    use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_IFIRSTDAYOFWEEK};
    use windows::core::PCWSTR;
    let mut buf = [0u16; 4];
    // SAFETY: a sized buffer; a null locale name means the user default.
    let n = unsafe { GetLocaleInfoEx(PCWSTR::null(), LOCALE_IFIRSTDAYOFWEEK, Some(&mut buf)) };
    if n < 2 {
        return 0;
    }
    char::from_u32(u32::from(buf[0]))
        .and_then(|c| c.to_digit(10))
        .filter(|d| *d <= 6)
        .unwrap_or(0)
}

pub fn clock() -> Clock {
    use windows::Win32::Globalization::{
        ENUM_DATE_FORMATS_FLAGS, GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS,
    };
    use windows::Win32::System::SystemInformation::GetLocalTime;
    use windows::core::{PCWSTR, w};

    let read = |buf: &[u16], n: i32| {
        let n = (n.max(1) as usize - 1).min(buf.len());
        String::from_utf16_lossy(&buf[..n])
    };
    // SAFETY: buffers are sized and the SYSTEMTIME is a local value; a null
    // locale name means the user default.
    unsafe {
        let st = GetLocalTime();
        let mut tbuf = [0u16; 32];
        let n = GetTimeFormatEx(
            PCWSTR::null(),
            TIME_NOSECONDS,
            Some(&st),
            PCWSTR::null(),
            Some(&mut tbuf),
        );
        let time = if n > 0 {
            read(&tbuf, n)
        } else {
            format!("{:02}:{:02}", st.wHour, st.wMinute)
        };
        let mut dbuf = [0u16; 32];
        let n = GetDateFormatEx(
            PCWSTR::null(),
            ENUM_DATE_FORMATS_FLAGS(0),
            Some(&st),
            w!("ddd d"),
            Some(&mut dbuf),
            PCWSTR::null(),
        );
        let date = if n > 0 {
            read(&dbuf, n)
        } else {
            st.wDay.to_string()
        };
        Clock {
            time,
            date,
            secs_to_next_minute: 60 - u32::from(st.wSecond).min(59),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    pub percent: u8,
    pub charging: bool,
}

/// `None` on machines without a battery.
pub fn battery() -> Option<Battery> {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut st = SYSTEM_POWER_STATUS::default();
    // SAFETY: valid out pointer.
    unsafe { GetSystemPowerStatus(&mut st) }.ok()?;
    // 128 = no system battery, 255 = unknown status.
    if st.BatteryFlag & 128 != 0 || st.BatteryFlag == 255 || st.BatteryLifePercent > 100 {
        return None;
    }
    Some(Battery {
        percent: st.BatteryLifePercent,
        charging: st.ACLineStatus == 1,
    })
}

/// Plays the Windows "notification" sound (respects the user's sound scheme
/// and mute). Cosmetic, so a failure is only logged.
pub fn alert_sound() {
    // SAFETY: plain call with a sound-type constant.
    if let Err(e) = unsafe { MessageBeep(MB_ICONASTERISK) } {
        tracing::debug!("MessageBeep: {e}");
    }
}

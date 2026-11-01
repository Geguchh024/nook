//! One-shot helpers for Quick Controls. Call these from a worker thread.

use crate::registry::{self, Hive};
use crate::window::Hwnd;
use crate::{PlatformError, Result, Win32Context};
use std::process::Command as ProcessCommand;
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind as WinRadioKind, RadioState};
use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, PHYSICAL_MONITOR, SetMonitorBrightness,
};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    AudioSessionStateActive, DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2,
    IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, eMultimedia, eRender,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::core::Interface;

static HOST_WINDOW: AtomicIsize = AtomicIsize::new(0);

const PUSH_NOTIFICATIONS: &str = r"Software\Microsoft\Windows\CurrentVersion\PushNotifications";

/// Initializes WinRT on a controls worker thread.
pub fn initialize_worker() -> Result<()> {
    // SAFETY: called once at worker startup; the matching uninitialize runs
    // on the same thread when that worker exits.
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.api("RoInitialize(controls worker)")
}

/// Balances [`initialize_worker`] on the calling worker thread.
pub fn uninitialize_worker() {
    // SAFETY: caller initialized WinRT on this same worker thread.
    unsafe { RoUninitialize() };
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub wifi: Option<bool>,
    pub bluetooth: Option<bool>,
    pub notifications_muted: bool,
    pub volume: Option<f32>,
    pub muted: Option<bool>,
    pub brightness: Option<f32>,
    pub outputs: Vec<AudioOutput>,
    pub audio_apps: Vec<AudioApp>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioOutput {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioApp {
    pub name: String,
    pub icon_path: Option<String>,
    pub icon: Option<AudioAppIcon>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioAppIcon {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA pixels from the Windows shell icon provider.
    pub rgba: Vec<u8>,
}

/// Sets the Nook window whose display is the target for brightness actions.
pub fn set_host_window(hwnd: Hwnd) {
    HOST_WINDOW.store(hwnd.raw(), Ordering::Relaxed);
}

fn host_monitor() -> Option<windows::Win32::Graphics::Gdi::HMONITOR> {
    let raw = HOST_WINDOW.load(Ordering::Relaxed);
    (raw != 0).then(|| crate::monitor::native_handle(Hwnd::from_raw(raw)))
}

fn with_physical_monitors<T>(
    f: impl FnOnce(&[PHYSICAL_MONITOR]) -> Result<T>,
) -> Result<Option<T>> {
    let Some(monitor) = host_monitor() else {
        return Ok(None);
    };
    let mut count = 0;
    // SAFETY: valid monitor handle and output pointer.
    unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut count) }
        .api("GetNumberOfPhysicalMonitorsFromHMONITOR")?;
    if count == 0 {
        return Ok(None);
    }
    let mut physical = vec![PHYSICAL_MONITOR::default(); count as usize];
    // SAFETY: the vector has exactly the requested number of entries.
    unsafe { GetPhysicalMonitorsFromHMONITOR(monitor, &mut physical) }
        .api("GetPhysicalMonitorsFromHMONITOR")?;
    let result = f(&physical);
    // SAFETY: release every handle returned above, including when f fails.
    let cleanup = unsafe { DestroyPhysicalMonitors(&physical) }.api("DestroyPhysicalMonitors");
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(Some(value)),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

fn monitor_brightness(monitor: &PHYSICAL_MONITOR) -> Option<(u32, u32, u32)> {
    let (mut min, mut current, mut max) = (0, 0, 0);
    // SAFETY: output pointers are valid; handle belongs to the active monitor list.
    (unsafe { GetMonitorBrightness(monitor.hPhysicalMonitor, &mut min, &mut current, &mut max) }
        != 0)
        .then_some((min, current, max))
}

fn read_brightness() -> Option<f32> {
    with_physical_monitors(|monitors| {
        Ok(monitors
            .iter()
            .find_map(monitor_brightness)
            .map(|(min, value, max)| {
                if max <= min {
                    0.0
                } else {
                    (value.saturating_sub(min)) as f32 / (max - min) as f32
                }
            }))
    })
    .ok()
    .flatten()
    .flatten()
    .or_else(read_wmi_brightness)
}

fn run_wmi(script: &str) -> Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    use windows::Win32::System::Threading::CREATE_NO_WINDOW;

    let mut command = ProcessCommand::new("powershell.exe");
    command.creation_flags(CREATE_NO_WINDOW.0).args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ]);
    command
        .output()
        .map_err(|e| PlatformError::Other(format!("starting Windows brightness helper: {e}")))
}

fn read_wmi_brightness() -> Option<f32> {
    let output = run_wmi(
        "Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightness | Where-Object Active | Select-Object -First 1 -ExpandProperty CurrentBrightness",
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout)
        .ok()?
        .trim()
        .parse::<u8>()
        .ok()?;
    Some(value as f32 / 100.0)
}

fn set_wmi_brightness(value: f32) -> Result<bool> {
    let level = (value.clamp(0.0, 1.0) * 100.0).round() as u8;
    let script = format!(
        "$m = Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightnessMethods | Where-Object Active | Select-Object -First 1; if ($null -eq $m) {{ exit 2 }}; $r = Invoke-CimMethod -InputObject $m -MethodName WmiSetBrightness -Arguments @{{Timeout=0; Brightness={level}}}; if ($r.ReturnValue -ne 0) {{ exit 3 }}"
    );
    let output = run_wmi(&script)?;
    if output.status.success() {
        Ok(true)
    } else if output.status.code() == Some(2) {
        Ok(false)
    } else {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(PlatformError::Other(if reason.is_empty() {
            "Windows rejected the brightness change".into()
        } else {
            reason
        }))
    }
}

pub fn set_brightness(value: f32) -> Result<bool> {
    match set_ddc_brightness(value)? {
        Some(true) => Ok(true),
        Some(false) | None => set_wmi_brightness(value),
    }
}

/// Updates an external display through DDC/CI without invoking the slower
/// laptop-panel WMI fallback. Used for live preview while a slider is dragged.
pub fn set_brightness_preview(value: f32) -> Result<()> {
    match set_ddc_brightness(value)? {
        Some(true) => Ok(()),
        Some(false) | None => Ok(()),
    }
}

fn set_ddc_brightness(value: f32) -> Result<Option<bool>> {
    let set = with_physical_monitors(|monitors| {
        for monitor in monitors {
            if let Some((min, _, max)) = monitor_brightness(monitor) {
                let level = min + ((max - min) as f32 * value.clamp(0.0, 1.0)).round() as u32;
                // SAFETY: handle and range were queried from this physical monitor.
                if unsafe { SetMonitorBrightness(monitor.hPhysicalMonitor, level) } == 0 {
                    return Err(PlatformError::Other(
                        "the selected display rejected its brightness change".into(),
                    ));
                }
                return Ok(true);
            }
        }
        Ok(false)
    })?;
    Ok(set)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RadioKind {
    Wifi,
    Bluetooth,
}

impl RadioKind {
    fn windows(self) -> WinRadioKind {
        match self {
            RadioKind::Wifi => WinRadioKind::WiFi,
            RadioKind::Bluetooth => WinRadioKind::Bluetooth,
        }
    }
}

fn radio_state(kind: RadioKind) -> Option<bool> {
    if Radio::RequestAccessAsync().ok()?.join().ok()? != RadioAccessStatus::Allowed {
        return None;
    }
    let radios = Radio::GetRadiosAsync().ok()?.join().ok()?;
    for index in 0..radios.Size().ok()? {
        let radio = radios.GetAt(index).ok()?;
        if radio.Kind().ok()? == kind.windows() {
            return Some(radio.State().ok()? == RadioState::On);
        }
    }
    None
}

pub fn radio_enabled(kind: RadioKind) -> Option<bool> {
    radio_state(kind)
}

fn endpoint() -> Result<IAudioEndpointVolume> {
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .api("CoCreateInstance(MMDeviceEnumerator)")?;
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) }
        .api("GetDefaultAudioEndpoint")?;
    unsafe { device.Activate(CLSCTX_ALL, None) }.api("IMMDevice::Activate(IAudioEndpointVolume)")
}

fn allocated_wide(value: windows::core::PWSTR) -> Option<String> {
    // Windows allocates the returned PWSTR with CoTaskMemAlloc.
    let text = unsafe { value.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(value.0.cast::<core::ffi::c_void>().cast_const())) };
    text.filter(|text| !text.trim().is_empty())
}

fn output_name(device: &IMMDevice) -> Option<String> {
    let store = unsafe { device.OpenPropertyStore(STGM_READ) }.ok()?;
    let mut value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }.ok()?;
    let text = unsafe { PropVariantToStringAlloc(&value) }
        .ok()
        .and_then(allocated_wide);
    let _ = unsafe { PropVariantClear(&mut value) };
    text
}

fn output_devices() -> Vec<AudioOutput> {
    let enumerator: windows::core::Result<IMMDeviceEnumerator> =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) };
    let Ok(enumerator) = enumerator else {
        return Vec::new();
    };
    let Ok(default) = (unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) }) else {
        return Vec::new();
    };
    let default_id = unsafe { default.GetId() }.ok().and_then(allocated_wide);
    let Ok(devices) = (unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) })
    else {
        return Vec::new();
    };
    let Ok(count) = (unsafe { devices.GetCount() }) else {
        return Vec::new();
    };
    (0..count)
        .filter_map(|index| {
            let device = unsafe { devices.Item(index) }.ok()?;
            let id = unsafe { device.GetId() }.ok().and_then(allocated_wide)?;
            Some(AudioOutput {
                name: output_name(&device).unwrap_or_else(|| "Audio output".into()),
                is_default: default_id.as_deref() == Some(&id),
                id,
            })
        })
        .collect()
}

fn audio_apps() -> Vec<AudioApp> {
    let enumerator: windows::core::Result<IMMDeviceEnumerator> =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) };
    let Ok(enumerator) = enumerator else {
        return Vec::new();
    };
    let Ok(devices) = (unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) })
    else {
        return Vec::new();
    };
    let Ok(device_count) = (unsafe { devices.GetCount() }) else {
        return Vec::new();
    };
    let mut active_sessions = Vec::new();
    for device_index in 0..device_count {
        let Ok(device) = (unsafe { devices.Item(device_index) }) else {
            continue;
        };
        let Ok(manager) = (unsafe { device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) })
        else {
            continue;
        };
        let Ok(sessions) = (unsafe { manager.GetSessionEnumerator() }) else {
            continue;
        };
        let Ok(count) = (unsafe { sessions.GetCount() }) else {
            continue;
        };
        for index in 0..count {
            if let Ok(session) = unsafe { sessions.GetSession(index) }
                && unsafe { session.GetState() }.is_ok_and(|state| state == AudioSessionStateActive)
            {
                active_sessions.push(session);
            }
        }
    }
    let mut apps = Vec::new();
    for session in active_sessions {
        let Ok(session) = session.cast::<IAudioSessionControl2>() else {
            continue;
        };
        let process_id = unsafe { session.GetProcessId() }.unwrap_or_default();
        if process_id == 0 {
            continue;
        }
        let icon_path = unsafe { session.GetIconPath() }
            .ok()
            .and_then(allocated_wide);
        let name = unsafe { session.GetDisplayName() }
            .ok()
            .and_then(allocated_wide)
            .or_else(|| {
                icon_path
                    .as_deref()
                    .and_then(|path| path.rsplit(['\\', '/']).next())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| format!("Audio app ({process_id})"));
        apps.push(AudioApp {
            name,
            icon_path,
            icon: None,
        });
    }
    apps.sort_by_key(|app| app.name.to_lowercase());
    apps.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    let icon_paths: Vec<_> = apps
        .iter()
        .enumerate()
        .filter_map(|(index, app)| {
            let path = app.icon_path.as_deref()?.trim_start_matches('@');
            let path = path
                .split_once(',')
                .map_or(path, |(path, _)| path)
                .trim_matches('"');
            std::path::Path::new(path)
                .exists()
                .then(|| (index, path.to_owned()))
        })
        .collect();
    if icon_paths.is_empty() {
        return apps;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let Ok(worker) = std::thread::Builder::new()
        .name("nook-audio-icons".into())
        .spawn(move || {
            crate::files::init_thumbnail_thread();
            for (index, path) in icon_paths {
                if let Ok((width, height, rgba)) =
                    crate::files::thumbnail(std::path::Path::new(&path), 48)
                    && tx
                        .send((
                            index,
                            AudioAppIcon {
                                width,
                                height,
                                rgba,
                            },
                        ))
                        .is_err()
                {
                    break;
                }
            }
        })
    else {
        return apps;
    };
    for (index, icon) in rx {
        if let Some(app) = apps.get_mut(index) {
            app.icon = Some(icon);
        }
    }
    let _ = worker.join();
    apps
}

pub fn snapshot() -> Snapshot {
    let audio = endpoint().ok();
    Snapshot {
        wifi: radio_state(RadioKind::Wifi),
        bluetooth: radio_state(RadioKind::Bluetooth),
        notifications_muted: registry::read_dword(
            Hive::CurrentUser,
            PUSH_NOTIFICATIONS,
            "ToastEnabled",
        )
        .unwrap_or(1)
            == 0,
        volume: audio
            .as_ref()
            .and_then(|a| unsafe { a.GetMasterVolumeLevelScalar().ok() }),
        muted: audio
            .as_ref()
            .and_then(|a| unsafe { a.GetMute().ok() })
            .map(|b| b.as_bool()),
        brightness: read_brightness(),
        outputs: output_devices(),
        audio_apps: audio_apps(),
    }
}

pub fn set_radio(kind: RadioKind, on: bool) -> Result<bool> {
    if Radio::RequestAccessAsync()
        .api("Radio::RequestAccessAsync")?
        .join()
        .api("Radio::RequestAccessAsync.join")?
        != RadioAccessStatus::Allowed
    {
        return Ok(false);
    }
    let radios = Radio::GetRadiosAsync()
        .api("Radio::GetRadiosAsync")?
        .join()
        .api("Radio::GetRadiosAsync.join")?;
    for index in 0..radios.Size().api("IVectorView<Radio>::Size")? {
        let radio = radios.GetAt(index).api("IVectorView<Radio>::GetAt")?;
        if radio.Kind().api("Radio::Kind")? == kind.windows() {
            let state = if on { RadioState::On } else { RadioState::Off };
            let result = radio
                .SetStateAsync(state)
                .api("Radio::SetStateAsync")?
                .join()
                .api("Radio::SetStateAsync.join")?;
            if result == RadioAccessStatus::Allowed {
                return Ok(true);
            }
            return Err(crate::PlatformError::Other(
                "Windows denied the radio state change".into(),
            ));
        }
    }
    Ok(false)
}

pub fn set_volume(value: f32) -> Result<()> {
    unsafe {
        endpoint()?
            .SetMasterVolumeLevelScalar(value.clamp(0.0, 1.0), &windows::core::GUID::zeroed())
    }
    .api("IAudioEndpointVolume::SetMasterVolumeLevelScalar")
}

pub fn set_muted(muted: bool) -> Result<()> {
    unsafe { endpoint()?.SetMute(muted, &windows::core::GUID::zeroed()) }
        .api("IAudioEndpointVolume::SetMute")
}

pub fn set_notifications_muted(muted: bool) -> Result<()> {
    registry::write_dword(
        Hive::CurrentUser,
        PUSH_NOTIFICATIONS,
        "ToastEnabled",
        u32::from(!muted),
    )
}

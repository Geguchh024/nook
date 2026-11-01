//! One-shot helpers for Quick Controls. Call these from a worker thread.

use crate::registry::{self, Hive};
use crate::{Result, Win32Context};
use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind as WinRadioKind, RadioState};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eMultimedia, eRender};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};

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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub wifi: Option<bool>,
    pub bluetooth: Option<bool>,
    pub notifications_muted: bool,
    pub volume: Option<f32>,
    pub muted: Option<bool>,
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

fn endpoint() -> Result<IAudioEndpointVolume> {
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .api("CoCreateInstance(MMDeviceEnumerator)")?;
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) }
        .api("GetDefaultAudioEndpoint")?;
    unsafe { device.Activate(CLSCTX_ALL, None) }.api("IMMDevice::Activate(IAudioEndpointVolume)")
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

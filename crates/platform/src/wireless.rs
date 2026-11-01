//! User-triggered Wi-Fi scanning/connection and Bluetooth discovery/pairing.
//! Call these functions from a worker; none of them is used by the idle frame.

use crate::controls::{self, RadioKind};
use crate::{PlatformError, Result, Win32Context};
use crossbeam_channel::Sender;
use std::thread;
use std::time::Duration;
use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationUpdate, DeviceWatcher};
use windows::Foundation::TypedEventHandler;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::NetworkManagement::WiFi::{
    DOT11_AUTH_ALGO_RSNA_PSK, DOT11_AUTH_ALGO_WPA_PSK, WLAN_AVAILABLE_NETWORK,
    WLAN_AVAILABLE_NETWORK_CONNECTED, WLAN_CONNECTION_NOTIFICATION_DATA,
    WLAN_CONNECTION_PARAMETERS, WLAN_INTERFACE_INFO, WLAN_INTERFACE_INFO_LIST,
    WLAN_NOTIFICATION_ACM, WLAN_NOTIFICATION_SOURCE_ACM, WLAN_NOTIFICATION_SOURCE_NONE,
    WlanCloseHandle, WlanConnect, WlanEnumInterfaces, WlanFreeMemory, WlanGetAvailableNetworkList,
    WlanOpenHandle, WlanRegisterNotification, WlanScan, WlanSetProfile,
    dot11_BSS_type_infrastructure, wlan_connection_mode_profile,
    wlan_notification_acm_connection_complete, wlan_notification_acm_disconnected,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::core::{HSTRING, Interface, PCWSTR};

/// Wi-Fi network visible in the last user-requested scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiNetwork {
    pub ssid: String,
    pub signal: u32,
    pub secured: bool,
    pub password_supported: bool,
    pub saved_profile: Option<String>,
    pub connected: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WifiSnapshot {
    pub enabled: Option<bool>,
    pub current_ssid: Option<String>,
    pub networks: Vec<WifiNetwork>,
}

/// Named Bluetooth device, including pairing and current connection state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BluetoothDeviceInfo {
    pub id: String,
    pub name: String,
    pub paired: bool,
    pub connected: bool,
    /// Battery percentage if the device reports it through Windows.
    pub battery: Option<u8>,
    pub can_pair: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BluetoothSnapshot {
    pub enabled: Option<bool>,
    pub devices: Vec<BluetoothDeviceInfo>,
}

/// Initialize WinRT on a dedicated radio worker thread.
pub fn initialize_worker() -> Result<()> {
    // SAFETY: paired with uninitialize_worker on the same worker thread.
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.api("RoInitialize(wireless worker)")
}

pub fn uninitialize_worker() {
    // SAFETY: caller initialized WinRT on this same worker thread.
    unsafe { RoUninitialize() };
}

/// Owns the native Wi-Fi connection event registration. The callback only
/// forwards a short notice; it never touches egui or blocks the radio thread.
pub struct WifiWatcher {
    client: WlanClient,
    context: *mut Sender<String>,
}

unsafe impl Send for WifiWatcher {}

pub fn watch_wifi(sender: Sender<String>) -> Result<WifiWatcher> {
    let client = WlanClient::open()?;
    let context = Box::into_raw(Box::new(sender));
    // SAFETY: context remains allocated until WlanRegisterNotification is
    // removed in Drop. The callback is a process-lifetime native function.
    let result = unsafe {
        WlanRegisterNotification(
            client.0,
            WLAN_NOTIFICATION_SOURCE_ACM,
            false,
            Some(wifi_notification),
            Some(context.cast()),
            None,
            None,
        )
    };
    if result != 0 {
        // SAFETY: ownership has not escaped because registration failed.
        unsafe { drop(Box::from_raw(context)) };
        return Err(PlatformError::Code {
            call: "WlanRegisterNotification",
            code: result,
        });
    }
    Ok(WifiWatcher { client, context })
}

impl Drop for WifiWatcher {
    fn drop(&mut self) {
        // SAFETY: unregistering removes this client's callback before context is freed.
        let _ = unsafe {
            WlanRegisterNotification(
                self.client.0,
                WLAN_NOTIFICATION_SOURCE_NONE,
                false,
                None,
                None,
                None,
                None,
            )
        };
        // SAFETY: the box was created by watch_wifi and callbacks are unregistered.
        unsafe { drop(Box::from_raw(self.context)) };
    }
}

unsafe extern "system" fn wifi_notification(
    data: *mut windows::Win32::NetworkManagement::WiFi::L2_NOTIFICATION_DATA,
    context: *mut std::ffi::c_void,
) {
    if data.is_null() || context.is_null() {
        return;
    }
    // SAFETY: WLAN invokes the callback with valid notification/context pointers.
    let data = unsafe { &*data };
    if data.NotificationSource != WLAN_NOTIFICATION_SOURCE_ACM || data.pData.is_null() {
        return;
    }
    let kind = WLAN_NOTIFICATION_ACM(data.NotificationCode as i32);
    let connected = if kind == wlan_notification_acm_connection_complete {
        true
    } else if kind == wlan_notification_acm_disconnected {
        false
    } else {
        return;
    };
    // SAFETY: these notification codes carry WLAN_CONNECTION_NOTIFICATION_DATA.
    let info = unsafe { &*data.pData.cast::<WLAN_CONNECTION_NOTIFICATION_DATA>() };
    let len = (info.dot11Ssid.uSSIDLength as usize).min(info.dot11Ssid.ucSSID.len());
    let ssid = String::from_utf8_lossy(&info.dot11Ssid.ucSSID[..len]);
    let label = if ssid.is_empty() {
        String::from_utf16_lossy(&info.strProfileName)
            .trim_end_matches('\0')
            .to_owned()
    } else {
        ssid.to_string()
    };
    let message = if connected {
        format!("Connected to {label}")
    } else if label.is_empty() {
        "Wi-Fi disconnected".to_owned()
    } else {
        format!("Disconnected from {label}")
    };
    // SAFETY: context points to the live Sender owned by WifiWatcher.
    let sender = unsafe { &*context.cast::<Sender<String>>() };
    let _ = sender.send(message);
}

pub fn set_radio(kind: RadioKind, on: bool) -> Result<bool> {
    controls::set_radio(kind, on)
}

struct WlanClient(HANDLE);

impl WlanClient {
    fn open() -> Result<Self> {
        let mut negotiated = 0;
        let mut handle = HANDLE::default();
        // SAFETY: output pointers are valid and the API receives no optional data.
        let result = unsafe { WlanOpenHandle(2, None, &mut negotiated, &mut handle) };
        if result == 0 {
            Ok(Self(handle))
        } else {
            Err(PlatformError::Code {
                call: "WlanOpenHandle",
                code: result,
            })
        }
    }

    fn interfaces(&self) -> Result<Vec<WLAN_INTERFACE_INFO>> {
        let mut raw: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        // SAFETY: raw is a valid output pointer and the returned allocation is freed below.
        let result = unsafe { WlanEnumInterfaces(self.0, None, &mut raw) };
        if result != 0 {
            return Err(PlatformError::Code {
                call: "WlanEnumInterfaces",
                code: result,
            });
        }
        if raw.is_null() {
            return Ok(Vec::new());
        }
        // SAFETY: WLAN returns a flexible-array allocation with dwNumberOfItems entries.
        let interfaces = unsafe {
            let list = &*raw;
            let ptr = std::ptr::addr_of!(list.InterfaceInfo).cast::<WLAN_INTERFACE_INFO>();
            std::slice::from_raw_parts(ptr, list.dwNumberOfItems as usize).to_vec()
        };
        // SAFETY: raw is the allocation returned by WlanEnumInterfaces.
        unsafe { WlanFreeMemory(raw.cast()) };
        Ok(interfaces)
    }
}

impl Drop for WlanClient {
    fn drop(&mut self) {
        // SAFETY: this handle was returned by WlanOpenHandle.
        let _ = unsafe { WlanCloseHandle(self.0, None) };
    }
}

fn network_entries(
    client: &WlanClient,
    interface: &WLAN_INTERFACE_INFO,
) -> Result<Vec<WifiNetwork>> {
    let mut raw = std::ptr::null_mut();
    // SAFETY: output pointer is valid; result allocation is freed before return.
    let result = unsafe {
        WlanGetAvailableNetworkList(client.0, &interface.InterfaceGuid, 0, None, &mut raw)
    };
    if result != 0 {
        return Err(PlatformError::Code {
            call: "WlanGetAvailableNetworkList",
            code: result,
        });
    }
    if raw.is_null() {
        return Ok(Vec::new());
    }
    // SAFETY: WLAN returns a flexible-array allocation with dwNumberOfItems entries.
    let items = unsafe {
        let list = &*raw;
        let ptr = std::ptr::addr_of!(list.Network).cast::<WLAN_AVAILABLE_NETWORK>();
        std::slice::from_raw_parts(ptr, list.dwNumberOfItems as usize)
            .iter()
            .filter_map(|network| {
                let len =
                    (network.dot11Ssid.uSSIDLength as usize).min(network.dot11Ssid.ucSSID.len());
                let ssid = String::from_utf8_lossy(&network.dot11Ssid.ucSSID[..len]).to_string();
                (!ssid.is_empty()).then(|| {
                    let profile = String::from_utf16_lossy(&network.strProfileName);
                    let profile = profile.trim_end_matches('\0').to_owned();
                    WifiNetwork {
                        ssid,
                        signal: network.wlanSignalQuality.min(100),
                        secured: network.bSecurityEnabled.as_bool(),
                        password_supported: !network.bSecurityEnabled.as_bool()
                            || network.dot11DefaultAuthAlgorithm == DOT11_AUTH_ALGO_RSNA_PSK
                            || network.dot11DefaultAuthAlgorithm == DOT11_AUTH_ALGO_WPA_PSK,
                        saved_profile: (!profile.is_empty()).then_some(profile),
                        connected: network.dwFlags & WLAN_AVAILABLE_NETWORK_CONNECTED != 0,
                    }
                })
            })
            .collect::<Vec<_>>()
    };
    // SAFETY: raw is the allocation returned by WlanGetAvailableNetworkList.
    unsafe { WlanFreeMemory(raw.cast()) };
    Ok(items)
}

/// Requests one scan, waits for the driver to refresh its cache, then returns
/// the available networks. It is only called after a user opens Wi-Fi details
/// or presses Refresh.
pub fn scan_wifi() -> Result<WifiSnapshot> {
    let enabled = controls::radio_enabled(RadioKind::Wifi);
    if enabled != Some(true) {
        return Ok(WifiSnapshot {
            enabled,
            ..WifiSnapshot::default()
        });
    }
    let client = WlanClient::open()?;
    let interfaces = client.interfaces()?;
    let Some(interface) = interfaces.first() else {
        return Ok(WifiSnapshot {
            enabled,
            ..WifiSnapshot::default()
        });
    };
    // SAFETY: interface GUID belongs to the enumerated adapter.
    let result = unsafe { WlanScan(client.0, &interface.InterfaceGuid, None, None, None) };
    if result != 0 {
        return Err(PlatformError::Code {
            call: "WlanScan",
            code: result,
        });
    }
    thread::sleep(Duration::from_millis(1600));
    let mut networks = network_entries(&client, interface)?;
    networks.sort_by(|a, b| b.signal.cmp(&a.signal).then_with(|| a.ssid.cmp(&b.ssid)));
    let current_ssid = networks
        .iter()
        .find(|network| network.connected)
        .map(|network| network.ssid.clone());
    Ok(WifiSnapshot {
        enabled,
        current_ssid,
        networks,
    })
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Connects to an open or WPA2-Personal network, saving a current-user WLAN
/// profile so Windows can reconnect later. Unsupported security schemes fail
/// explicitly so the UI can offer Windows network settings as fallback.
pub fn connect_wifi(ssid: &str, password: Option<&str>) -> Result<()> {
    let client = WlanClient::open()?;
    let interfaces = client.interfaces()?;
    let interface = interfaces
        .first()
        .ok_or_else(|| PlatformError::Other("no Wi-Fi adapter is available".into()))?;
    let networks = network_entries(&client, interface)?;
    let network = networks
        .iter()
        .find(|network| network.ssid == ssid)
        .ok_or_else(|| PlatformError::Other("network is no longer in scan range".into()))?;
    let profile_name = network
        .saved_profile
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or(ssid);
    if network.saved_profile.as_deref().is_none_or(str::is_empty) {
        let escaped_ssid = xml_escape(ssid);
        if network.secured {
            let Some(password) = password else {
                return Err(PlatformError::Other("this network needs a password".into()));
            };
            if password.len() < 8 || password.len() > 63 {
                return Err(PlatformError::Other(
                    "Wi-Fi passwords must contain 8–63 characters".into(),
                ));
            }
            if !network.password_supported {
                return Err(PlatformError::Other(
                    "this Wi-Fi security type must be connected through Windows Settings".into(),
                ));
            }
            // This profile form is for WPA2-Personal only; enterprise/WEP
            // profiles should be created through Windows Settings.
            let xml = format!(
                "<security><authEncryption><authentication>WPA2PSK</authentication><encryption>AES</encryption><useOneX>false</useOneX></authEncryption><sharedKey><keyType>passPhrase</keyType><protected>false</protected><keyMaterial>{}</keyMaterial></sharedKey></security>",
                xml_escape(password)
            );
            if network.secured && network.saved_profile.is_none() {
                let profile_xml = format!(
                    "<?xml version=\"1.0\"?><WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\"><name>{escaped_ssid}</name><SSIDConfig><SSID><name>{escaped_ssid}</name></SSID><nonBroadcast>false</nonBroadcast></SSIDConfig><connectionType>ESS</connectionType><connectionMode>auto</connectionMode><MSM><security>{}</security></MSM></WLANProfile>",
                    xml.trim_start_matches("<security>")
                        .trim_end_matches("</security>")
                );
                let profile = HSTRING::from(profile_xml);
                let mut reason = 0;
                // SAFETY: pointers refer to null-terminated HSTRING allocations for the call.
                let result = unsafe {
                    WlanSetProfile(
                        client.0,
                        &interface.InterfaceGuid,
                        0,
                        PCWSTR(profile.as_ptr()),
                        PCWSTR::null(),
                        true,
                        None,
                        &mut reason,
                    )
                };
                if result != 0 {
                    return Err(PlatformError::Code {
                        call: "WlanSetProfile",
                        code: result,
                    });
                }
            }
        } else {
            let profile_xml = format!(
                "<?xml version=\"1.0\"?><WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\"><name>{escaped_ssid}</name><SSIDConfig><SSID><name>{escaped_ssid}</name></SSID><nonBroadcast>false</nonBroadcast></SSIDConfig><connectionType>ESS</connectionType><connectionMode>auto</connectionMode><MSM><security><authEncryption><authentication>open</authentication><encryption>none</encryption><useOneX>false</useOneX></authEncryption></security></MSM></WLANProfile>"
            );
            let profile = HSTRING::from(profile_xml);
            let mut reason = 0;
            // SAFETY: pointers refer to null-terminated HSTRING allocations for the call.
            let result = unsafe {
                WlanSetProfile(
                    client.0,
                    &interface.InterfaceGuid,
                    0,
                    PCWSTR(profile.as_ptr()),
                    PCWSTR::null(),
                    true,
                    None,
                    &mut reason,
                )
            };
            if result != 0 {
                return Err(PlatformError::Code {
                    call: "WlanSetProfile",
                    code: result,
                });
            }
        }
    }

    let profile = HSTRING::from(profile_name);
    let params = WLAN_CONNECTION_PARAMETERS {
        wlanConnectionMode: wlan_connection_mode_profile,
        strProfile: PCWSTR(profile.as_ptr()),
        pDot11Ssid: std::ptr::null_mut(),
        pDesiredBssidList: std::ptr::null_mut(),
        dot11BssType: dot11_BSS_type_infrastructure,
        dwFlags: 0,
    };
    // SAFETY: profile and params remain valid for the synchronous API call.
    let result = unsafe { WlanConnect(client.0, &interface.InterfaceGuid, &params, None) };
    if result != 0 {
        return Err(PlatformError::Code {
            call: "WlanConnect",
            code: result,
        });
    }
    Ok(())
}

fn device_infos(selector: &HSTRING) -> Result<Vec<BluetoothDeviceInfo>> {
    let battery_property = HSTRING::from("System.Devices.BatteryLife");
    let devices = DeviceInformation::FindAllAsyncAqsFilter(selector)
        .api("DeviceInformation::FindAllAsync")?
        .join()
        .api("DeviceInformation::FindAllAsync.join")?;
    let mut found = Vec::new();
    for index in 0..devices.Size().api("DeviceInformationCollection::Size")? {
        let device = devices
            .GetAt(index)
            .api("DeviceInformationCollection::GetAt")?;
        let pairing = device.Pairing().api("DeviceInformation::Pairing")?;
        let id = device.Id().api("DeviceInformation::Id")?.to_string();
        let name = device.Name().api("DeviceInformation::Name")?.to_string();
        let paired = pairing.IsPaired().unwrap_or(false);
        let can_pair = pairing.CanPair().unwrap_or(false);
        let battery = device
            .Properties()
            .ok()
            .and_then(|properties| properties.Lookup(&battery_property).ok())
            .and_then(|value| value.cast::<windows::Foundation::IPropertyValue>().ok())
            .and_then(|value| value.GetUInt8().ok())
            .filter(|value| *value <= 100);
        let connected = BluetoothDevice::FromIdAsync(&HSTRING::from(&id))
            .ok()
            .and_then(|op| op.join().ok())
            .and_then(|radio| radio.ConnectionStatus().ok())
            .is_some_and(|state| state == BluetoothConnectionStatus::Connected);
        found.push(BluetoothDeviceInfo {
            id,
            name,
            paired,
            connected,
            battery,
            can_pair,
        });
    }
    Ok(found)
}

/// Enumerates paired and nearby devices only when the user opens Bluetooth
/// details or asks for a refresh.
pub fn scan_bluetooth() -> Result<BluetoothSnapshot> {
    let enabled = controls::radio_enabled(RadioKind::Bluetooth);
    if enabled != Some(true) {
        return Ok(BluetoothSnapshot {
            enabled,
            devices: Vec::new(),
        });
    }
    let paired = BluetoothDevice::GetDeviceSelectorFromPairingState(true)
        .api("BluetoothDevice::GetDeviceSelectorFromPairingState(paired)")?;
    let nearby = BluetoothDevice::GetDeviceSelectorFromPairingState(false)
        .api("BluetoothDevice::GetDeviceSelectorFromPairingState(nearby)")?;
    let mut all = device_infos(&paired)?;
    all.extend(device_infos(&nearby)?);
    all.sort_by_key(|device| device.name.to_lowercase());
    all.dedup_by(|a, b| a.id == b.id);
    Ok(BluetoothSnapshot {
        enabled,
        devices: all,
    })
}

/// Pairs an unpaired Bluetooth device, or removes an existing pairing.
pub fn set_bluetooth_pairing(id: &str, paired: bool) -> Result<()> {
    let device = DeviceInformation::CreateFromIdAsync(&HSTRING::from(id))
        .api("DeviceInformation::CreateFromIdAsync")?
        .join()
        .api("DeviceInformation::CreateFromIdAsync.join")?;
    let pairing = device.Pairing().api("DeviceInformation::Pairing")?;
    if paired {
        let result = pairing
            .UnpairAsync()
            .api("DeviceInformationPairing::UnpairAsync")?
            .join()
            .api("DeviceInformationPairing::UnpairAsync.join")?;
        if !result.Status().is_ok_and(|status| {
            status == windows::Devices::Enumeration::DeviceUnpairingResultStatus::Unpaired
                || status
                    == windows::Devices::Enumeration::DeviceUnpairingResultStatus::AlreadyUnpaired
        }) {
            return Err(PlatformError::Other(
                "Windows could not remove the Bluetooth pairing".into(),
            ));
        }
    } else {
        let result = pairing
            .PairAsync()
            .api("DeviceInformationPairing::PairAsync")?
            .join()
            .api("DeviceInformationPairing::PairAsync.join")?;
        if !result.Status().is_ok_and(|status| {
            status == windows::Devices::Enumeration::DevicePairingResultStatus::Paired
                || status == windows::Devices::Enumeration::DevicePairingResultStatus::AlreadyPaired
        }) {
            return Err(PlatformError::Other(
                "Windows could not pair with this Bluetooth device".into(),
            ));
        }
    }
    Ok(())
}

/// Opens the Windows-owned Bluetooth UI for connecting an already paired
/// device; Windows does not expose one generic connect call for every profile.
pub fn open_bluetooth_settings() -> Result<()> {
    crate::shell::open("ms-settings:bluetooth")
}

/// Starts an event-driven watcher for Bluetooth devices which are connected.
/// It emits only connect/disconnect transitions after initial enumeration.
pub struct BluetoothWatcher {
    watcher: DeviceWatcher,
    added: i64,
    removed: i64,
    completed: i64,
}

pub fn watch_bluetooth(sender: Sender<String>) -> Result<BluetoothWatcher> {
    let selector = BluetoothDevice::GetDeviceSelectorFromConnectionStatus(
        BluetoothConnectionStatus::Connected,
    )
    .api("BluetoothDevice::GetDeviceSelectorFromConnectionStatus")?;
    let watcher = DeviceInformation::CreateWatcherAqsFilter(&selector)
        .api("DeviceInformation::CreateWatcher")?;
    let initialized = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let names = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
        String,
        String,
    >::new()));

    let added_names = names.clone();
    let added_initialized = initialized.clone();
    let added_sender = sender.clone();
    let added = watcher
        .Added(&TypedEventHandler::new(
            move |_, info: windows::core::Ref<DeviceInformation>| {
                let Ok(info) = info.ok() else {
                    return Ok(());
                };
                let id = info.Id().map(|id| id.to_string()).unwrap_or_default();
                let name = info.Name().map(|name| name.to_string()).unwrap_or_default();
                if !id.is_empty() {
                    if let Ok(mut cache) = added_names.lock() {
                        cache.insert(id, name.clone());
                    }
                    if added_initialized.load(std::sync::atomic::Ordering::Acquire) {
                        let label = if name.is_empty() {
                            "Bluetooth device"
                        } else {
                            &name
                        };
                        let _ = added_sender.send(format!("{label} connected"));
                    }
                }
                Ok(())
            },
        ))
        .api("DeviceWatcher::Added")?;

    let removed_names = names.clone();
    let removed_initialized = initialized.clone();
    let removed_sender = sender.clone();
    let removed = watcher
        .Removed(&TypedEventHandler::new(
            move |_, update: windows::core::Ref<DeviceInformationUpdate>| {
                let Ok(update) = update.ok() else {
                    return Ok(());
                };
                let id = update.Id().map(|id| id.to_string()).unwrap_or_default();
                if let Ok(mut cache) = removed_names.lock()
                    && let Some(name) = cache.remove(&id)
                    && removed_initialized.load(std::sync::atomic::Ordering::Acquire)
                {
                    let label = if name.is_empty() {
                        "Bluetooth device"
                    } else {
                        &name
                    };
                    let _ = removed_sender.send(format!("{label} disconnected"));
                }
                Ok(())
            },
        ))
        .api("DeviceWatcher::Removed")?;

    let completed_initialized = initialized.clone();
    let completed = watcher
        .EnumerationCompleted(&TypedEventHandler::new(move |_, _| {
            completed_initialized.store(true, std::sync::atomic::Ordering::Release);
            Ok(())
        }))
        .api("DeviceWatcher::EnumerationCompleted")?;
    watcher.Start().api("DeviceWatcher::Start")?;
    Ok(BluetoothWatcher {
        watcher,
        added,
        removed,
        completed,
    })
}

impl Drop for BluetoothWatcher {
    fn drop(&mut self) {
        let _ = self.watcher.Stop();
        let _ = self.watcher.RemoveAdded(self.added);
        let _ = self.watcher.RemoveRemoved(self.removed);
        let _ = self.watcher.RemoveEnumerationCompleted(self.completed);
    }
}

#[cfg(test)]
mod tests {
    use super::xml_escape;

    #[test]
    fn escapes_profile_text_for_wlan_xml() {
        assert_eq!(xml_escape("a&b<\"'"), "a&amp;b&lt;&quot;&apos;");
    }
}

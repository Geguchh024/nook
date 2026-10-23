//! Minimal registry access (values only, no key enumeration).

use crate::{Result, check, wide};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::PCWSTR;

#[derive(Clone, Copy, Debug)]
pub enum Hive {
    CurrentUser,
    LocalMachine,
}

impl Hive {
    fn key(self) -> HKEY {
        match self {
            Hive::CurrentUser => HKEY_CURRENT_USER,
            Hive::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }
}

/// Reads a `REG_DWORD`; `None` if missing or of another type.
pub fn read_dword(hive: Hive, subkey: &str, value: &str) -> Option<u32> {
    let (k, v) = (wide(subkey), wide(value));
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: data/size point at a u32 we own.
    let err = unsafe {
        RegGetValueW(
            hive.key(),
            PCWSTR(k.as_ptr()),
            PCWSTR(v.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    err.is_ok().then_some(data)
}

/// Reads a `REG_SZ`; `None` if missing.
pub fn read_string(hive: Hive, subkey: &str, value: &str) -> Option<String> {
    let (k, v) = (wide(subkey), wide(value));
    let mut size = 0u32;
    // SAFETY: first call only queries the size.
    let err = unsafe {
        RegGetValueW(
            hive.key(),
            PCWSTR(k.as_ptr()),
            PCWSTR(v.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if err.is_err() || size == 0 {
        return None;
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: buf holds `size` bytes.
    let err = unsafe {
        RegGetValueW(
            hive.key(),
            PCWSTR(k.as_ptr()),
            PCWSTR(v.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if err.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Writes a `REG_SZ`, creating the key if needed.
pub fn write_string(hive: Hive, subkey: &str, value: &str, data: &str) -> Result<()> {
    let (k, v, d) = (wide(subkey), wide(value), wide(data));
    // SAFETY: d is a valid null-terminated UTF-16 buffer of the given size.
    let err = unsafe {
        RegSetKeyValueW(
            hive.key(),
            PCWSTR(k.as_ptr()),
            PCWSTR(v.as_ptr()),
            REG_SZ.0,
            Some(d.as_ptr() as *const _),
            (d.len() * 2) as u32,
        )
    };
    check("RegSetKeyValueW", err)
}

/// Deletes a value; a missing value is not an error.
pub fn delete_value(hive: Hive, subkey: &str, value: &str) -> Result<()> {
    let (k, v) = (wide(subkey), wide(value));
    // SAFETY: valid null-terminated strings.
    let err = unsafe { RegDeleteKeyValueW(hive.key(), PCWSTR(k.as_ptr()), PCWSTR(v.as_ptr())) };
    if err == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    check("RegDeleteKeyValueW", err)
}

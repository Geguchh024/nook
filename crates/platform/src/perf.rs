//! System load for the system monitor: CPU, memory, network traffic,
//! disks, uptime and per-process usage. Every call is a cheap snapshot;
//! rates come from the caller diffing two of them.

use crate::{Result, Win32Context, wide};
use windows::Win32::Foundation::{CloseHandle, FILETIME};
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::SystemInformation::{
    GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, GetSystemTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::core::PCWSTR;

fn ticks(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

/// Cumulative CPU time of the whole machine, in 100 ns ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub idle: u64,
    /// Kernel + user time (kernel time includes idle time).
    pub total: u64,
}

impl CpuTimes {
    /// Busy share (0..1) between `prev` and `self`.
    pub fn usage_since(&self, prev: &CpuTimes) -> f32 {
        let total = self.total.saturating_sub(prev.total);
        let idle = self.idle.saturating_sub(prev.idle);
        if total == 0 {
            return 0.0;
        }
        (1.0 - idle as f64 / total as f64).clamp(0.0, 1.0) as f32
    }
}

pub fn cpu_times() -> Result<CpuTimes> {
    let (mut idle, mut kernel, mut user) = Default::default();
    // SAFETY: valid out pointers.
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }
        .api("GetSystemTimes")?;
    Ok(CpuTimes {
        idle: ticks(idle),
        total: ticks(kernel) + ticks(user),
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    pub used: u64,
    pub total: u64,
}

pub fn memory() -> Result<Memory> {
    let mut st = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: a sized out struct.
    unsafe { GlobalMemoryStatusEx(&mut st) }.api("GlobalMemoryStatusEx")?;
    Ok(Memory {
        used: st.ullTotalPhys.saturating_sub(st.ullAvailPhys),
        total: st.ullTotalPhys,
    })
}

/// Bytes received and sent so far by the physical network adapters
/// (virtual adapters and filters would count the same traffic twice).
pub fn net_bytes() -> Result<(u64, u64)> {
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: the table is allocated by the call and freed below; rows are
    // read within `NumEntries`.
    unsafe {
        crate::check("GetIfTable2", GetIfTable2(&mut table))?;
        if table.is_null() {
            return Ok((0, 0));
        }
        let n = (*table).NumEntries as usize;
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), n);
        let (mut down, mut up) = (0u64, 0u64);
        for row in rows {
            // Bit 0: HardwareInterface. 24: software loopback.
            let hardware = row.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
            if hardware && row.Type != 24 {
                down += row.InOctets;
                up += row.OutOctets;
            }
        }
        FreeMibTable(table.cast());
        Ok((down, up))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disk {
    /// e.g. `C:`.
    pub name: String,
    pub free: u64,
    pub total: u64,
}

/// Fixed (internal) drives with their free space.
pub fn disks() -> Vec<Disk> {
    // SAFETY: plain bitmask query.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .filter_map(|i| {
            let root = format!("{}:\\", (b'A' + i) as char);
            let w = wide(&root);
            // SAFETY: null-terminated root path; 3 = DRIVE_FIXED.
            if unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) } != 3 {
                return None;
            }
            let (mut free, mut total) = (0u64, 0u64);
            // SAFETY: valid out pointers.
            unsafe {
                GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), Some(&mut free), Some(&mut total), None)
            }
            .ok()?;
            Some(Disk {
                name: root.trim_end_matches('\\').to_owned(),
                free,
                total,
            })
        })
        .collect()
}

/// Time since Windows started.
pub fn uptime() -> std::time::Duration {
    // SAFETY: plain query.
    std::time::Duration::from_millis(unsafe { GetTickCount64() })
}

/// One running process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    /// Executable name, e.g. `chrome.exe`.
    pub name: String,
    /// Kernel + user time so far, 100 ns ticks.
    pub cpu: u64,
    /// Working set, bytes.
    pub memory: u64,
}

/// Every process we may query (system processes of other sessions are
/// listed with zero usage).
pub fn processes() -> Result<Vec<Process>> {
    // SAFETY: a snapshot handle we close; entries are sized structs.
    unsafe {
        let snap =
            CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).api("CreateToolhelp32Snapshot")?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut out = Vec::with_capacity(256);
        let mut ok = Process32FirstW(snap, &mut entry).is_ok();
        while ok {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            let pid = entry.th32ProcessID;
            let (mut cpu, mut memory) = (0, 0);
            if pid != 0
                && let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            {
                let (mut c, mut e, mut k, mut u) = Default::default();
                if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).is_ok() {
                    cpu = ticks(k) + ticks(u);
                }
                let mut pmc = PROCESS_MEMORY_COUNTERS {
                    cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
                    ..Default::default()
                };
                if K32GetProcessMemoryInfo(h, &mut pmc, pmc.cb).as_bool() {
                    memory = pmc.WorkingSetSize as u64;
                }
                let _ = CloseHandle(h);
            }
            out.push(Process {
                pid,
                name,
                cpu,
                memory,
            });
            ok = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_usage_from_two_samples() {
        let a = CpuTimes {
            idle: 100,
            total: 200,
        };
        let b = CpuTimes {
            idle: 130,
            total: 300,
        };
        assert!((b.usage_since(&a) - 0.7).abs() < 1e-6);
        assert_eq!(a.usage_since(&a), 0.0);
    }

    #[test]
    fn snapshots_work() {
        assert!(memory().unwrap().total > 0);
        assert!(cpu_times().unwrap().total > 0);
        assert!(processes().unwrap().len() > 3);
        assert!(!disks().is_empty());
    }
}

//! Whether the network is metered and the computer runs on battery, for the tracker's seeding pauses
//! (`dissent_core::tracker::seeding`). Read every minute and handed to the engine. A platform that cannot tell reports
//! unmetered and on mains power, so nothing pauses by mistake.

use std::time::Duration;

use dissent_core::tracker::seeding::DeviceConditions;

/// How often the conditions are read.
const EVERY: Duration = Duration::from_secs(60);

/// Reads the conditions now.
pub fn read() -> DeviceConditions {
    let metered = metered();
    DeviceConditions { metered: metered.unwrap_or(false), metered_known: metered.is_some(), on_battery: on_battery() }
}

/// Reports the conditions to the running engine now and every minute after, for as long as the app runs. The engine is
/// looked up each time, so a restarted engine gets them too.
pub fn watch(app: tauri::AppHandle) {
    use tauri::Manager;
    tauri::async_runtime::spawn(async move {
        loop {
            let now = tauri::async_runtime::spawn_blocking(read).await.unwrap_or_default();
            let engine = app.state::<crate::commands::AppState>().engine.read().await.clone();
            if let Some(engine) = engine {
                if engine.device_conditions() != now {
                    engine.set_device_conditions(now);
                }
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

#[cfg(windows)]
fn on_battery() -> bool {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut status: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: the pointer is to a local of the type the call fills.
    let ok = unsafe { GetSystemPowerStatus(&mut status) } != 0;
    // 0 is offline (on battery), 1 online, 255 unknown.
    ok && status.ACLineStatus == 0
}

/// `GetNetworkConnectivityHint` is Windows 10 2004 and later, so it is looked up at run time: on an older Windows the
/// app still starts, and the network reads as unmetered.
#[cfg(windows)]
fn metered() -> Option<bool> {
    use windows_sys::Win32::Networking::WinSock::{NetworkConnectivityCostHintFixed, NetworkConnectivityCostHintVariable, NL_NETWORK_CONNECTIVITY_HINT};
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    type Hint = unsafe extern "system" fn(*mut NL_NETWORK_CONNECTIVITY_HINT) -> u32;
    let name: Vec<u16> = "iphlpapi.dll\0".encode_utf16().collect();
    // SAFETY: a NUL-terminated wide string; the module stays loaded for the process, which is what we want.
    let module = unsafe { LoadLibraryW(name.as_ptr()) };
    if module.is_null() {
        return None;
    }
    // SAFETY: a NUL-terminated name; the result is checked before use.
    let proc = (unsafe { GetProcAddress(module, c"GetNetworkConnectivityHint".as_ptr().cast()) })?;
    // SAFETY: the export has this signature on every Windows that has it.
    let hint: Hint = unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, Hint>(proc) };
    let mut out: NL_NETWORK_CONNECTIVITY_HINT = unsafe { std::mem::zeroed() };
    // SAFETY: the pointer is to a local of the type the call fills.
    if unsafe { hint(&mut out) } != 0 {
        return None;
    }
    Some(out.ConnectivityCost == NetworkConnectivityCostHintFixed || out.ConnectivityCost == NetworkConnectivityCostHintVariable)
}

/// On battery when no mains adapter is online and some battery is present.
#[cfg(target_os = "linux")]
fn on_battery() -> bool {
    let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") else {
        return false;
    };
    let (mut mains_online, mut battery) = (false, false);
    for entry in entries.flatten() {
        let path = entry.path();
        let kind = std::fs::read_to_string(path.join("type")).unwrap_or_default();
        match kind.trim() {
            "Mains" | "USB" => mains_online |= std::fs::read_to_string(path.join("online")).is_ok_and(|v| v.trim() == "1"),
            "Battery" => battery |= std::fs::read_to_string(path.join("present")).map_or(true, |v| v.trim() == "1"),
            _ => {}
        }
    }
    battery && !mains_online
}

#[cfg(target_os = "macos")]
fn on_battery() -> bool {
    std::process::Command::new("pmset")
        .args(["-g", "batt"])
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains("'Battery Power'"))
}

/// Not detected on these platforms yet.
#[cfg(not(windows))]
fn metered() -> Option<bool> {
    None
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn on_battery() -> bool {
    false
}

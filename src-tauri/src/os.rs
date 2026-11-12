//! Windows helpers: foreground window, process list, vendor-software conflicts, launching, lock/sleep.

use std::sync::mpsc::Sender;
use std::time::Duration;
use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, SW_SHOWNORMAL};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Foreground {
    pub exe: Option<String>,
    pub elevated: bool,
}

/// Some(true/false) when the token could be read.
fn token_elevated(process: HANDLE) -> Option<bool> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let mut elev = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elev as *mut _ as *mut core::ffi::c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        let _ = CloseHandle(token);
        ok.ok()?;
        Some(elev.TokenIsElevated != 0)
    }
}

pub fn is_self_elevated() -> bool {
    static SELF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SELF.get_or_init(|| token_elevated(unsafe { GetCurrentProcess() }).unwrap_or(false))
}

pub fn query_foreground() -> Foreground {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return Foreground::default();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return Foreground::default();
        }
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            // cannot even query it → most likely an elevated/protected process
            return Foreground { exe: None, elevated: !is_self_elevated() };
        };
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let exe = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size)
            .ok()
            .map(|_| {
                let full = String::from_utf16_lossy(&buf[..size as usize]);
                full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string()
            });
        let elevated = if is_self_elevated() { false } else { token_elevated(h).unwrap_or(true) };
        let _ = CloseHandle(h);
        Foreground { exe, elevated }
    }
}

/// Polls the foreground window; sends only on change. Ends when the receiver is gone.
pub fn spawn_foreground_watcher(interval: Duration, tx: Sender<Foreground>) {
    std::thread::Builder::new()
        .name("foreground".into())
        .spawn(move || {
            let mut last: Option<Foreground> = None;
            loop {
                let cur = query_foreground();
                if last.as_ref() != Some(&cur) {
                    if tx.send(cur.clone()).is_err() {
                        return;
                    }
                    last = Some(cur);
                }
                std::thread::sleep(interval);
            }
        })
        .expect("spawn foreground watcher");
}

/// Lower-cased, unique, sorted exe names of running processes.
pub fn list_process_names() -> Vec<String> {
    let mut names = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return names };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                names.push(String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase());
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    names.retain(|n| !n.is_empty());
    names.sort();
    names.dedup();
    names
}

pub fn find_conflicts(names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|n| ["fifine", "streamdock", "ampligame", "hotspot"].iter().any(|v| n.contains(v)))
        .cloned()
        .collect()
}

/// Programs/scripts are spawned directly; anything else (URL, document, folder) goes to the shell.
pub fn launch_target(target: &str, args: &[String]) {
    let lower = target.to_lowercase();
    if lower.ends_with(".exe") || lower.ends_with(".bat") || lower.ends_with(".cmd") {
        if let Err(e) = std::process::Command::new(target).args(args).spawn() {
            log::error!("launching {target} failed: {e}");
        }
        return;
    }
    let file = HSTRING::from(target);
    let params = HSTRING::from(args.join(" "));
    let r = unsafe { ShellExecuteW(None, &HSTRING::from("open"), &file, &params, None, SW_SHOWNORMAL) };
    if (r.0 as isize) <= 32 {
        log::error!("opening {target} failed (code {})", r.0 as isize);
    }
}

pub fn run_system(command: &str) {
    match command {
        "lock" => {
            if let Err(e) = unsafe { LockWorkStation() } {
                log::error!("lock failed: {e}");
            }
        }
        "sleep" => {
            // Direct call: rundll32 passes garbage BOOLEANs to SetSuspendState and may hibernate.
            let ok = unsafe { windows::Win32::System::Power::SetSuspendState(false, false, false) };
            if !ok {
                log::error!("sleep failed");
            }
        }
        other => log::warn!("unknown system command {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_conflicts_picks_vendor_software() {
        let names: Vec<String> = ["explorer.exe", "fifine control deck.exe", "streamdock.exe", "wow.exe"].iter().map(|s| s.to_string()).collect();
        assert_eq!(find_conflicts(&names), vec!["fifine control deck.exe".to_string(), "streamdock.exe".to_string()]);
    }

    #[test]
    fn process_list_is_lowercase_unique_sorted() {
        let names = list_process_names();
        assert!(names.len() > 5);
        assert!(names.iter().all(|n| *n == n.to_lowercase()));
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(names, sorted);
        assert!(names.iter().any(|n| n == "explorer.exe" || n == "svchost.exe"));
    }

    #[test]
    fn foreground_query_does_not_panic() {
        let _ = query_foreground();
    }
}

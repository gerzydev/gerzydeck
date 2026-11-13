//! One-time, opt-in ETW permission for the bundled PresentMon.
//! Token access and persisted group membership are separate: log off/on activates new membership.

use windows::core::{w, BOOL, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::NetworkManagement::NetManagement::{NetApiBufferFree, NetUserGetLocalGroups, LG_INCLUDE_INDIRECT, LOCALGROUP_USERS_INFO_0, MAX_PREFERRED_LENGTH};
use windows::Win32::Security::Authentication::Identity::{GetUserNameExW, NameSamCompatible};
use windows::Win32::Security::{CheckTokenMembership, CreateWellKnownSid, DuplicateToken, GetTokenInformation, LookupAccountSidW, SecurityIdentification, TokenElevation, PSID, SID_NAME_USE, TOKEN_DUPLICATE, TOKEN_ELEVATION, TOKEN_QUERY, WinBuiltinPerfLoggingUsersSid};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FpsPermission {
    pub active: bool,
    pub added: bool,
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) { unsafe { let _ = CloseHandle(self.0); } }
}
struct NetBuffer(*mut u8);
impl Drop for NetBuffer {
    fn drop(&mut self) { if !self.0.is_null() { unsafe { NetApiBufferFree(Some(self.0.cast())); } } }
}

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

fn current_user() -> Result<String, String> {
    unsafe {
        let mut len = 0;
        GetUserNameExW(NameSamCompatible, None, &mut len);
        if len == 0 || len > 32768 { return Err(windows::core::Error::from_thread().to_string()); }
        let mut name = vec![0u16; len as usize];
        if !GetUserNameExW(NameSamCompatible, Some(PWSTR(name.as_mut_ptr())), &mut len) {
            return Err(windows::core::Error::from_thread().to_string());
        }
        let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
        Ok(String::from_utf16_lossy(&name[..end]))
    }
}

fn check_permission() -> Result<FpsPermission, String> {
    unsafe {
        // Aligned storage large enough for SECURITY_MAX_SID_SIZE; RID 559 is locale independent.
        let mut storage = [0u32; 17];
        let sid = PSID(storage.as_mut_ptr().cast());
        let mut len = std::mem::size_of_val(&storage) as u32;
        CreateWellKnownSid(WinBuiltinPerfLoggingUsersSid, None, Some(sid), &mut len).map_err(|e| e.to_string())?;
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY | TOKEN_DUPLICATE, &mut token).map_err(|e| e.to_string())?;
        let token = Handle(token);
        let mut elevation = TOKEN_ELEVATION::default();
        let mut needed = 0;
        GetTokenInformation(token.0, TokenElevation, Some((&mut elevation as *mut TOKEN_ELEVATION).cast()), std::mem::size_of_val(&elevation) as u32, &mut needed).map_err(|e| e.to_string())?;
        // CheckTokenMembership requires an impersonation token. Duplicate the process
        // token explicitly rather than accidentally checking a worker's thread token.
        let mut impersonation = HANDLE::default();
        DuplicateToken(token.0, SecurityIdentification, &mut impersonation).map_err(|e| e.to_string())?;
        let impersonation = Handle(impersonation);
        let mut member = BOOL::default();
        CheckTokenMembership(Some(impersonation.0), sid, &mut member).map_err(|e| e.to_string())?;
        let active = elevation.TokenIsElevated != 0 || member.as_bool();

        let (mut name_len, mut domain_len) = (0, 0);
        let mut use_kind = SID_NAME_USE::default();
        let _ = LookupAccountSidW(PCWSTR::null(), sid, None, &mut name_len, None, &mut domain_len, &mut use_kind);
        if name_len == 0 || name_len > 32768 || domain_len > 32768 { return Err(windows::core::Error::from_thread().to_string()); }
        let mut group = vec![0u16; name_len as usize];
        let mut domain = vec![0u16; domain_len as usize];
        LookupAccountSidW(PCWSTR::null(), sid, Some(PWSTR(group.as_mut_ptr())), &mut name_len, Some(PWSTR(domain.as_mut_ptr())), &mut domain_len, &mut use_kind).map_err(|e| e.to_string())?;
        let group = String::from_utf16_lossy(&group[..name_len as usize]);
        let user = wide(&current_user()?);
        let mut buffer = std::ptr::null_mut();
        let (mut entries, mut total) = (0, 0);
        let code = NetUserGetLocalGroups(PCWSTR::null(), PCWSTR(user.as_ptr()), 0, LG_INCLUDE_INDIRECT, &mut buffer, MAX_PREFERRED_LENGTH, &mut entries, &mut total);
        let buffer = NetBuffer(buffer);
        if code != 0 { return Err(format!("NetUserGetLocalGroups: {code}")); }
        if entries > 0 && buffer.0.is_null() { return Err("NetUserGetLocalGroups: empty result".into()); }
        let mut added = false;
        for i in 0..entries as usize {
            let entry = &*buffer.0.cast::<LOCALGROUP_USERS_INFO_0>().add(i);
            let name = entry.lgrui0_name.to_string().map_err(|e| e.to_string())?;
            if name.eq_ignore_ascii_case(&group) { added = true; break; }
        }
        Ok(FpsPermission { active, added })
    }
}

/// A PowerShell single-quoted literal: interpolation and command delimiters stay data.
fn ps_literal(value: &str) -> String { format!("'{}'", value.replace("'", "''")) }

fn grant_script(user: &str) -> String {
    format!("$ErrorActionPreference='Stop'; try {{ Add-LocalGroupMember -SID 'S-1-5-32-559' -Member {} -ErrorAction Stop; exit 0 }} catch {{ if ($_.CategoryInfo.Reason -eq 'MemberExistsException') {{ exit 0 }}; exit 1 }}", ps_literal(user))
}

/// Quote one native Windows argument, including embedded quotes and trailing backslashes.
fn command_arg(value: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in value.chars() {
        if c == '\\' { slashes += 1; continue; }
        if c == '"' {
            out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
        } else {
            out.extend(std::iter::repeat_n('\\', slashes));
        }
        slashes = 0;
        out.push(c);
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

fn grant_parameters(user: &str) -> String {
    format!("-NoProfile -NonInteractive -WindowStyle Hidden -Command {}", command_arg(&grant_script(user)))
}

fn grant() -> Result<(), String> {
    use windows::Win32::Foundation::{ERROR_CANCELLED, WAIT_OBJECT_0};
    use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    if check_permission()?.added { return Ok(()); }
    let user = current_user()?;
    // Resolve the system PowerShell executable, never a PATH/current-directory substitute.
    let mut system = vec![0u16; 32768];
    let len = unsafe { GetSystemDirectoryW(Some(&mut system)) } as usize;
    if len == 0 || len >= system.len() { return Err(windows::core::Error::from_thread().to_string()); }
    let path = std::path::PathBuf::from(String::from_utf16_lossy(&system[..len])).join("WindowsPowerShell").join("v1.0").join("powershell.exe");
    let file = wide(&path.to_string_lossy());
    let params = wide(&grant_parameters(&user));
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).ok().map_err(|e| e.to_string())?;
        struct Com;
        impl Drop for Com { fn drop(&mut self) { unsafe { CoUninitialize(); } } }
        let _com = Com;
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: w!("runas"), lpFile: PCWSTR(file.as_ptr()), lpParameters: PCWSTR(params.as_ptr()),
            nShow: SW_HIDE.0, ..Default::default()
        };
        if let Err(e) = ShellExecuteExW(&mut info) {
            return Err(if e.code() == windows::core::HRESULT::from_win32(ERROR_CANCELLED.0) {
                crate::i18n::tr("Potvrzení správce bylo zrušeno.", "Administrator approval was canceled.").into()
            } else { e.to_string() });
        }
        if info.hProcess.is_invalid() { return Err(crate::i18n::tr("Nepodařilo se spustit nastavení oprávnění FPS.", "Could not start FPS permission setup.").into()); }
        let process = Handle(info.hProcess);
        if WaitForSingleObject(process.0, INFINITE) != WAIT_OBJECT_0 { return Err(windows::core::Error::from_thread().to_string()); }
        let mut code = 0;
        GetExitCodeProcess(process.0, &mut code).map_err(|e| e.to_string())?;
        if code != 0 { return Err(crate::trf!("Přidání do skupiny pro FPS selhalo (kód {code}).", "Adding FPS group membership failed (code {code}).")); }
    }
    Ok(())
}

#[tauri::command]
pub async fn fps_permission() -> Result<FpsPermission, String> {
    tauri::async_runtime::spawn_blocking(check_permission).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn grant_fps_permission() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(grant).await.map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn powershell_user_is_a_literal_even_with_apostrophes_and_metacharacters() {
        assert_eq!(ps_literal("DOMAIN\\O'Brien"), "'DOMAIN\\O''Brien'");
        assert_eq!(ps_literal("a'; $(whoami); '"), "'a''; $(whoami); '''");
        let script = grant_script("PC\\O'Brien");
        assert!(script.contains("-SID 'S-1-5-32-559' -Member 'PC\\O''Brien'"));
        assert!(script.contains("MemberExistsException"));
        assert!(script.ends_with("exit 1 }"));
    }
    #[test]
    fn native_argument_quotes_preserve_quotes_and_backslashes() {
        assert_eq!(command_arg("plain"), "\"plain\"");
        assert_eq!(command_arg("a\"b"), "\"a\\\"b\"");
        assert_eq!(command_arg("a\\"), "\"a\\\\\"");
        assert_eq!(command_arg(""), "\"\"");
    }
    #[test]
    fn parameters_use_no_profile_hidden_command_and_fixed_group() {
        let params = grant_parameters("PC\\Žluťoučký");
        assert!(params.starts_with("-NoProfile -NonInteractive -WindowStyle Hidden -Command \""));
        assert!(params.contains("S-1-5-32-559"));
        assert!(params.contains("PC\\Žluťoučký"));
        assert!(!params.contains("-ExecutionPolicy"));
    }
    #[test]
    fn native_command_line_round_trips_one_script_argument() {
        use windows::Win32::Foundation::{LocalFree, HLOCAL};
        use windows::Win32::UI::Shell::CommandLineToArgvW;
        for script in [grant_script("DOMAIN\\O'Brien"), grant_script("PC\\name\"; $(echo test)"), "a\\\"b\\".into()] {
            let command = wide(&format!("powershell.exe {}", command_arg(&script)));
            unsafe {
                let mut count = 0;
                let args = CommandLineToArgvW(PCWSTR(command.as_ptr()), &mut count);
                assert!(!args.is_null());
                assert_eq!(count, 2);
                let recovered = (*args.add(1)).to_string().unwrap();
                LocalFree(Some(HLOCAL(args.cast())));
                assert_eq!(recovered, script);
            }
        }
    }

}

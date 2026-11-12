//! Keyboard output. Keys go out as scan codes so games see them like a real keyboard.

use crate::scancodes::scancode;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE, VIRTUAL_KEY,
};

pub trait Keyboard: Send + Sync {
    fn down(&self, code: &str);
    fn up(&self, code: &str);
    fn tap_vk(&self, vk: u16);
    fn type_text(&self, text: &str);
}

pub struct SendInputKeyboard;

/// Pause between typed characters. Without it the new Windows 11 Notepad (and other
/// WinUI apps) turned "ěščř" into "řřřř" — it reads VK_PACKET characters too late.
pub const TEXT_CHAR_DELAY: std::time::Duration = std::time::Duration::from_millis(4);

fn input(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn send_all(inputs: &[INPUT]) {
    // SAFETY: fully initialised INPUTs with the correct struct size.
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        log::warn!("SendInput was blocked (elevated window in foreground?)");
    }
}

fn send(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) {
    send_all(&[input(vk, scan, flags)]);
}

fn scan(code: &str, up: bool) {
    let Some((sc, ext)) = scancode(code) else {
        log::warn!("unknown key code {code}");
        return;
    };
    let mut flags = KEYEVENTF_SCANCODE;
    if ext {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    send(0, sc, flags);
}

impl Keyboard for SendInputKeyboard {
    fn down(&self, code: &str) {
        scan(code, false);
    }
    fn up(&self, code: &str) {
        scan(code, true);
    }
    fn tap_vk(&self, vk: u16) {
        send_all(&[input(vk, 0, KEYEVENTF_EXTENDEDKEY), input(vk, 0, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP)]);
    }
    fn type_text(&self, text: &str) {
        for (i, unit) in text.encode_utf16().enumerate() {
            if i > 0 {
                std::thread::sleep(TEXT_CHAR_DELAY);
            }
            send_all(&[input(0, unit, KEYEVENTF_UNICODE), input(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP)]);
        }
    }
}

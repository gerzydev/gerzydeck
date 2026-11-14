//! Mutes the user's physical microphone (a Windows capture endpoint) while the soundboard plays
//! into the virtual microphone, so people hear the sound and not the room.
//!
//! The mute is the endpoint's own mute (IAudioEndpointVolume::SetMute), the same switch as in
//! Windows sound settings, so Sonar / Discord / games all get silence from that device.
//!
//! Safety: if the app dies while the microphone is muted, it would stay muted. While muted, a
//! marker file `mic-muted.json` in the data dir names the device; `restore_after_crash` (app start)
//! and `unmute` remove it again.
//!
//! Contract (used by sound.rs; keep these signatures):
//! - `list_inputs()`: active capture devices by friendly name, same names as
//!   `cpal::Device::name()` for input devices (PKEY_Device_FriendlyName).
//! - `mute(data_dir, name)`: mutes the device if it is not muted already. Returns `Ok(true)` when
//!   this call muted it (marker written), `Ok(false)` when it was already muted by someone else
//!   (nothing to undo later), `Err` when the device is missing or COM fails.
//! - `unmute(data_dir, name)`: unmutes the device and removes the marker.
//! - `restore_after_crash(data_dir)`: if the marker exists, unmutes the device it names and removes it.

use std::path::Path;

pub fn list_inputs() -> Vec<String> {
    todo!("Codex: active capture endpoints, friendly names")
}

pub fn mute(data_dir: &Path, name: &str) -> Result<bool, String> {
    let _ = (data_dir, name);
    todo!("Codex: IAudioEndpointVolume::SetMute(TRUE) unless already muted; write the marker")
}

pub fn unmute(data_dir: &Path, name: &str) -> Result<(), String> {
    let _ = (data_dir, name);
    todo!("Codex: IAudioEndpointVolume::SetMute(FALSE); remove the marker")
}

pub fn restore_after_crash(data_dir: &Path) {
    let _ = data_dir;
    todo!("Codex: unmute the device named in mic-muted.json, remove the file")
}

//! Tauri commands: thin wrappers that turn editor calls into engine messages.
//! All waiting commands are `async`: sync Tauri commands run on the main thread, and
//! blocking it while the engine talks to the tray deadlocked every save for 5 s.

use crate::config::Config;
use crate::engine::{Msg, State};
use base64::Engine as _;
use std::sync::mpsc::{channel, Sender};
use std::time::Duration;
use tauri::ipc::{InvokeBody, Request};
use tauri_plugin_autostart::ManagerExt;

pub struct EngineTx(pub Sender<Msg>);

fn ask<T>(tx: &EngineTx, make: impl FnOnce(Sender<T>) -> Msg) -> Result<T, String> {
    let (r, rx) = channel();
    tx.0.send(make(r)).map_err(|_| crate::i18n::tr("Jádro programu neběží", "The program core is not running").to_string())?;
    rx.recv_timeout(Duration::from_secs(5)).map_err(|_| crate::i18n::tr("Jádro programu neodpovídá", "The program core is not responding").to_string())
}

fn tell(tx: &EngineTx, msg: Msg) -> Result<(), String> {
    tx.0.send(msg).map_err(|_| crate::i18n::tr("Jádro programu neběží", "The program core is not running").to_string())
}

#[tauri::command]
pub async fn get_config(tx: tauri::State<'_, EngineTx>) -> Result<Config, String> {
    ask(&tx, Msg::GetConfig)
}

#[tauri::command]
pub async fn get_state(tx: tauri::State<'_, EngineTx>) -> Result<State, String> {
    ask(&tx, Msg::GetState)
}

#[tauri::command]
pub async fn set_config(tx: tauri::State<'_, EngineTx>, config: serde_json::Value) -> Result<Config, String> {
    ask(&tx, |r| Msg::SetConfig(config, r))?
}

#[tauri::command]
pub async fn swap_keys(tx: tauri::State<'_, EngineTx>, profile_id: String, a: u8, b: u8) -> Result<Config, String> {
    ask(&tx, |r| Msg::SwapKeys { profile_id, a, b, reply: r })?
}

#[tauri::command]
pub async fn set_active_profile(tx: tauri::State<'_, EngineTx>, id: String) -> Result<(), String> {
    tell(&tx, Msg::SetActiveProfile(id))
}

#[tauri::command]
pub async fn set_calibrating(tx: tauri::State<'_, EngineTx>, on: bool) -> Result<(), String> {
    tell(&tx, Msg::SetCalibrating(on))
}

/// Raw body: `invoke('import_image', uint8Array)`.
/// Any image → boot logo in the deck's flash (takes ~7 s: the deck needs pauses).
#[tauri::command]
pub async fn set_boot_logo(tx: tauri::State<'_, EngineTx>, request: Request<'_>) -> Result<(), String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err(crate::i18n::tr("Očekávám data obrázku", "Expected image data").into());
    };
    let jpeg = crate::render::boot_logo_jpeg(bytes).map_err(|e| e.to_string())?;
    let (r, rx) = channel();
    tx.0.send(Msg::SetBootLogo { jpeg, reply: r }).map_err(|_| crate::i18n::tr("Jádro programu neběží", "The program core is not running").to_string())?;
    rx.recv_timeout(Duration::from_secs(60)).map_err(|_| crate::i18n::tr("Deck neodpověděl", "The deck did not answer").to_string())?
}

#[tauri::command]
pub async fn import_image(tx: tauri::State<'_, EngineTx>, request: Request<'_>) -> Result<String, String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err(crate::i18n::tr("Očekávám data obrázku", "Expected image data").into());
    };
    let bytes = bytes.clone();
    ask(&tx, |r| Msg::ImportImage(bytes, r))?
}

#[tauri::command]
pub async fn image_data_url(id: String) -> Option<String> {
    let png = crate::store::Store::new(crate::store::data_dir()).read_image(&id)?;
    Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)))
}

#[tauri::command]
pub async fn list_processes() -> Vec<String> {
    crate::os::list_process_names()
}

#[tauri::command]
pub fn list_scancodes() -> Vec<&'static str> {
    crate::scancodes::all_codes()
}

#[tauri::command]
pub async fn get_autostart(app: tauri::AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_autostart(app: tauri::AppHandle, on: bool) -> Result<(), String> {
    let al = app.autolaunch();
    if on { al.enable() } else { al.disable() }.map_err(|e| e.to_string())
}

// ---------- soundboard ----------

pub struct SoundTx(pub Sender<crate::sound::SoundCmd>);

/// Raw body: `invoke('import_sound', uint8Array)` → stored sound id.
#[tauri::command]
pub async fn import_sound(request: Request<'_>) -> Result<String, String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err(crate::i18n::tr("Očekávám data zvuku", "Expected sound data").into());
    };
    crate::sound::import(&crate::store::data_dir(), bytes)
}

#[tauri::command]
pub async fn list_audio_outputs() -> Vec<String> {
    crate::sound::list_outputs()
}

/// The device "auto" picks for the microphone right now.
#[tauri::command]
pub async fn auto_mic_device() -> Option<String> {
    crate::sound::auto_mic(&crate::sound::list_outputs())
}

/// Editor test button: only to the user's own output, never into the microphone.
#[tauri::command]
pub async fn preview_sound(tx: tauri::State<'_, SoundTx>, sound: String, volume: u8) -> Result<(), String> {
    let cmd = crate::sound::SoundCmd::Press { sound, me: volume, mic: 0, mode: crate::sound::Mode::Restart };
    tx.0.send(cmd).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn stop_sounds(tx: tauri::State<'_, SoundTx>) -> Result<(), String> {
    tx.0.send(crate::sound::SoundCmd::StopAll).map_err(|e| e.to_string())
}

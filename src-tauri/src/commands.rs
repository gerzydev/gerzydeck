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
    tx.0.send(make(r)).map_err(|_| "Jádro programu neběží".to_string())?;
    rx.recv_timeout(Duration::from_secs(5)).map_err(|_| "Jádro programu neodpovídá".to_string())
}

fn tell(tx: &EngineTx, msg: Msg) -> Result<(), String> {
    tx.0.send(msg).map_err(|_| "Jádro programu neběží".to_string())
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
#[tauri::command]
pub async fn import_image(tx: tauri::State<'_, EngineTx>, request: Request<'_>) -> Result<String, String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("Očekávám data obrázku".into());
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

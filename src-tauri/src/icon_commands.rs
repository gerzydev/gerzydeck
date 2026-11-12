//! Tauri commands for the icon library and icon packs.

use crate::icons::{IconPack, IconStore, LibraryIcon, PackIcon};
use base64::Engine as _;
use tauri::ipc::{InvokeBody, Request};

fn store() -> IconStore {
    IconStore::new(IconStore::default_root())
}

fn raw<'a>(request: &'a Request<'_>, what: &str) -> Result<&'a [u8], String> {
    match request.body() {
        InvokeBody::Raw(bytes) => Ok(bytes),
        _ => Err(crate::trf!("Očekávám {what}", "Expected {what}")),
    }
}

#[tauri::command]
pub async fn icons_library() -> Vec<LibraryIcon> {
    store().list_library()
}

/// Raw image bytes; the name travels base64-encoded in the `x-name` header.
#[tauri::command]
pub async fn icons_upload(request: Request<'_>) -> Result<LibraryIcon, String> {
    let bytes = raw(&request, crate::i18n::tr("obrázek", "an image"))?;
    let name = request
        .headers()
        .get("x-name")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| base64::engine::general_purpose::STANDARD.decode(v).ok())
        .and_then(|v| String::from_utf8(v).ok())
        .unwrap_or_default();
    store().add_upload(bytes, &name)
}

#[tauri::command]
pub async fn icons_delete(id: String) -> Result<(), String> {
    store().delete_library(&id)
}

#[tauri::command]
pub async fn icon_packs() -> Vec<IconPack> {
    store().list_packs()
}

#[tauri::command]
pub async fn icon_pack_icons(id: String) -> Result<Vec<PackIcon>, String> {
    store().pack_icons(&id)
}

#[tauri::command]
pub async fn icon_pack_import(request: Request<'_>) -> Result<IconPack, String> {
    store().import_pack(raw(&request, crate::i18n::tr("balík ikon", "an icon pack"))?)
}

#[tauri::command]
pub async fn icon_pack_delete(id: String) -> Result<(), String> {
    store().delete_pack(&id)
}

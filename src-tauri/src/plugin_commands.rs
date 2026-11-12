//! Tauri commands for the Pluginy page and property inspectors.

use crate::commands::EngineTx;
use crate::engine::Msg;
use crate::plugins::catalog::{self, CatalogEntry, Source};
use crate::plugins::install;
use crate::plugins::link::{HostCmd, PiInfo, PluginSummary};
use serde::Serialize;
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;
use std::time::Duration;
use tauri::ipc::{InvokeBody, Request};

pub struct HostTx(pub Mutex<Sender<HostCmd>>);

pub struct PluginNet {
    pub client: reqwest::Client,
    pub catalog: Mutex<Vec<CatalogEntry>>,
}

impl PluginNet {
    pub fn new() -> Self {
        let client = reqwest::Client::builder().user_agent("GerzysDeck").timeout(Duration::from_secs(60)).build().unwrap_or_default();
        Self { client, catalog: Mutex::new(vec![]) }
    }
}

impl Default for PluginNet {
    fn default() -> Self {
        Self::new()
    }
}

fn host(tx: &HostTx, cmd: HostCmd) -> Result<(), String> {
    tx.0.lock().unwrap().send(cmd).map_err(|_| crate::i18n::tr("Hostitel pluginů neběží", "The plugin host is not running").to_string())
}

fn source_key(s: Source) -> &'static str {
    match s {
        Source::OpenAction => "openAction",
        Source::ElgatoArchive => "elgatoArchive",
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogRow {
    id: String,
    name: String,
    author: String,
    source: &'static str,
    icon_url: String,
}

#[derive(Serialize)]
pub struct CatalogResult {
    entries: Vec<CatalogRow>,
    errors: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetRow {
    name: String,
    url: String,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct InstallResult {
    installed: Option<String>,
    choose: Vec<AssetRow>,
}

#[tauri::command]
pub async fn plugins_list(tx: tauri::State<'_, HostTx>) -> Result<Vec<PluginSummary>, String> {
    let (r, rx) = channel();
    host(&tx, HostCmd::ListPlugins(r))?;
    rx.recv_timeout(Duration::from_secs(5)).map_err(|_| crate::i18n::tr("Hostitel pluginů neodpovídá", "The plugin host is not responding").to_string())
}

#[tauri::command]
pub async fn plugins_catalog(net: tauri::State<'_, PluginNet>) -> Result<CatalogResult, String> {
    let (entries, errors) = catalog::fetch_all(&net.client).await;
    let rows = entries
        .iter()
        .map(|e| CatalogRow { id: e.id.clone(), name: e.name.clone(), author: e.author.clone(), source: source_key(e.source), icon_url: e.icon_url.clone() })
        .collect();
    *net.catalog.lock().unwrap() = entries;
    Ok(CatalogResult { entries: rows, errors })
}

async fn download(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let resp = client.get(url).send().await.map_err(|e| crate::trf!("Stažení selhalo: {e}", "Download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(crate::trf!("Stažení selhalo: HTTP {}", "Download failed: HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| crate::trf!("Stažení selhalo: {e}", "Download failed: {e}"))?;
    if bytes.len() > 300 * 1024 * 1024 {
        return Err(crate::i18n::tr("Plugin je větší než 300 MB", "The plugin is larger than 300 MB").into());
    }
    Ok(bytes.to_vec())
}

fn install_bytes(tx: &HostTx, bytes: &[u8], fallback_id: Option<&str>) -> Result<String, String> {
    if let Some(id) = fallback_id {
        host(tx, HostCmd::Remove { id: id.to_string() })?; // stop the old version before replacing files
        std::thread::sleep(Duration::from_millis(300));
    }
    let plugin = install::install_from_zip(bytes, &crate::plugins::plugins_dir(), fallback_id).map_err(|e| e.to_string())?;
    host(tx, HostCmd::Reload { id: plugin.id.clone() })?;
    Ok(plugin.id)
}

#[tauri::command]
pub async fn plugins_install(
    tx: tauri::State<'_, HostTx>,
    net: tauri::State<'_, PluginNet>,
    id: String,
    asset_url: Option<String>,
) -> Result<InstallResult, String> {
    let entry = net.catalog.lock().unwrap().iter().find(|e| e.id == id).cloned().ok_or(crate::i18n::tr("Plugin není v katalogu", "The plugin is not in the catalogue"))?;
    let url = match asset_url {
        Some(u) => u,
        None => {
            let assets = catalog::resolve_download(&net.client, &entry).await.map_err(|e| e.to_string())?;
            match assets.len() {
                0 => return Err(crate::i18n::tr("Plugin nemá soubor ke stažení", "The plugin has no file to download").into()),
                1 => assets[0].url.clone(),
                _ => {
                    return Ok(InstallResult { installed: None, choose: assets.into_iter().map(|a| AssetRow { name: a.name, url: a.url }).collect() })
                }
            }
        }
    };
    let bytes = download(&net.client, &url).await?;
    let id = install_bytes(&tx, &bytes, Some(&entry.id))?;
    Ok(InstallResult { installed: Some(id), choose: vec![] })
}

/// Raw body: `invoke('plugins_install_file', uint8Array)`.
#[tauri::command]
pub async fn plugins_install_file(tx: tauri::State<'_, HostTx>, request: Request<'_>) -> Result<String, String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err(crate::i18n::tr("Očekávám soubor pluginu", "Expected a plugin file").into());
    };
    install_bytes(&tx, bytes, None)
}

#[tauri::command]
pub async fn plugins_uninstall(tx: tauri::State<'_, HostTx>, id: String) -> Result<(), String> {
    host(&tx, HostCmd::Remove { id: id.clone() })?;
    std::thread::sleep(Duration::from_millis(300));
    install::uninstall(&crate::plugins::plugins_dir(), &id).map_err(|e| format!("Odinstalace selhala: {e}"))
}

#[tauri::command]
pub async fn pi_open(tx: tauri::State<'_, EngineTx>, profile_id: String, key: u8) -> Result<Option<PiInfo>, String> {
    let (r, rx) = channel();
    tx.0.send(Msg::OpenPi { profile_id, key, reply: r }).map_err(|_| crate::i18n::tr("Jádro programu neběží", "The program core is not running").to_string())?;
    rx.recv_timeout(Duration::from_secs(5)).map_err(|_| crate::i18n::tr("Hostitel pluginů neodpovídá", "The plugin host is not responding").to_string())
}

#[tauri::command]
pub async fn pi_close(tx: tauri::State<'_, HostTx>, context: String) -> Result<(), String> {
    host(&tx, HostCmd::PiClose { context })
}

#[tauri::command]
pub async fn open_plugin_log(id: String) -> Result<(), String> {
    let _ = id;
    let dir = crate::store::data_dir().join("logs").join("plugins");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    crate::os::launch_target(&dir.to_string_lossy(), &[]);
    Ok(())
}

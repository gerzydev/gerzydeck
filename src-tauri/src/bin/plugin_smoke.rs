//! Opt-in network/real-process smoke harness. Ordinary tests never run the harness.
//!
//! Run from the repository root:
//! `cargo run --manifest-path src-tauri/Cargo.toml --bin plugin_smoke -- .superpowers/plugin-smoke`
//! Add `--only <plugin-id>` after the artifact directory for a single target.
//! `--action <uuid> --press` exercises a specific action. `--capture-node-browser`
//! intercepts Node browser commands in the isolated installed copy only.
//! Archives/source URLs are cached once; every run installs fresh isolated copies,
//! observes each for 30 seconds, stops its process tree and preserves JSON/log artifacts.
//! Logs combine stdout and stderr, so their individual provenance cannot be recovered.

use anyhow::{Context, Result};
use gerzys_deck::plugins::{
    catalog::{resolve_download, CatalogEntry, Source},
    host::{spawn_host, HostDeps, Launcher, LoadedPlugin, DEVICE_ID},
    install::install_from_zip,
    link::{HostCmd, KeyCtx, PluginEvent, PluginStatus},
    process::Supervisor,
    protocol::ActionCtx,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Default, Serialize)]
struct Observation {
    launched: bool,
    registered: bool,
    set_image: bool,
    set_title: bool,
    unknown: Vec<String>,
}

fn summarize(id: &str, diagnostics: &[Value]) -> Observation {
    let mut row = Observation::default();
    let mut unknown = BTreeSet::new();
    for event in diagnostics.iter().filter(|e| e["plugin"] == id) {
        match event["kind"].as_str() {
            Some("launched") => row.launched = true,
            Some("registered") => row.registered = true,
            Some("inbound") if event["classification"] == "known" => {
                row.set_image |= event["event"] == "setImage";
                row.set_title |= event["event"] == "setTitle";
            }
            Some("inbound") if event["classification"] == "unknown" => {
                if let Some(name) = event["event"].as_str() {
                    unknown.insert(name.to_owned());
                }
            }
            _ => {}
        }
    }
    row.unknown = unknown.into_iter().collect();
    row
}

struct SmokeLogger {
    diagnostics: Arc<Mutex<Vec<Value>>>,
    file: Mutex<File>,
    started: Instant,
}

impl log::Log for SmokeLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target() == "plugin_wire"
            || metadata.target() == "plugin_process"
            || metadata.target() == module_path!()
            || (metadata.target().starts_with("gerzys_deck::plugins")
                && metadata.level() <= log::Level::Info)
    }
    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let text = record.args().to_string();
        let elapsed_ms = self.started.elapsed().as_millis();
        if matches!(record.target(), "plugin_wire" | "plugin_process") {
            if let Ok(mut value) = serde_json::from_str::<Value>(&text) {
                value["elapsed_ms"] = json!(elapsed_ms);
                if let Ok(mut diagnostics) = self.diagnostics.lock() {
                    diagnostics.push(value);
                }
            }
        }
        if let Ok(mut file) = self.file.lock() {
            let _ = writeln!(
                file,
                "{elapsed_ms}ms {} {} {text}",
                record.level(),
                record.target()
            );
            let _ = file.flush();
        }
    }
    fn flush(&self) {
        if let Ok(mut file) = self.file.lock() {
            let _ = file.flush();
        }
    }
}

struct RecordingLauncher {
    inner: Supervisor,
    stopped: mpsc::Sender<()>,
}
impl Launcher for RecordingLauncher {
    fn start(&mut self, plugin: &LoadedPlugin, port: u16, uuid: &str, info: &str) {
        self.inner.start(plugin, port, uuid, info);
    }
    fn stop(&mut self, id: &str) {
        self.inner.stop(id);
    }
    fn stop_all(&mut self) {
        self.inner.stop_all();
        let _ = self.stopped.send(());
    }
    fn registered(&mut self, id: &str) {
        self.inner.mark_registered(id);
    }
    fn status(&self, id: &str) -> Option<PluginStatus> {
        Launcher::status(&self.inner, id)
    }
}

/// Makes process-tree shutdown observable even when an observation returns an error.
struct HostGuard {
    tx: mpsc::Sender<HostCmd>,
    stopped: Option<mpsc::Receiver<()>>,
}
impl HostGuard {
    fn shutdown(&mut self) -> Result<()> {
        let Some(stopped) = self.stopped.take() else {
            return Ok(());
        };
        self.tx
            .send(HostCmd::Shutdown)
            .context("host command channel closed")?;
        stopped
            .recv_timeout(Duration::from_secs(10))
            .context("host did not finish stopping processes")?;
        Ok(())
    }
}
impl Drop for HostGuard {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[derive(Serialize)]
struct SmokeResult {
    id: String,
    version: Option<String>,
    source_url: Option<String>,
    action: Option<String>,
    observation: Observation,
    host_events: Vec<String>,
    log_path: PathBuf,
    combined_output_bytes: u64,
    error: Option<String>,
    pressed: bool,
    browser_requests: Vec<String>,
}

fn select_action<'a>(actions: &'a [&str], wanted: Option<&str>) -> Option<&'a str> {
    match wanted {
        Some(wanted) => actions.iter().copied().find(|action| *action == wanted),
        None => actions.first().copied(),
    }
}

fn browser_requests(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            line.strip_prefix("SMOKE_BROWSER_REQUEST ")
                .and_then(|value| serde_json::from_str::<String>(value).ok())
        })
        .collect()
}

/// Smoke-only shim, scoped to a freshly installed temporary plugin copy. The
/// original archive, user's installation, process environment and host are untouched.
fn capture_node_browser(plugin: &LoadedPlugin) -> Result<()> {
    let gerzys_deck::plugins::manifest::Runtime::Node { script, .. } =
        plugin.manifest.runtime(&plugin.dir)?
    else {
        anyhow::bail!("--capture-node-browser requires a Node plugin");
    };
    let source = fs::read_to_string(&script)?;
    let shim = script
        .parent()
        .context("Node script parent")?
        .join("smoke-browser.cjs");
    fs::write(
        &shim,
        r#"const cp = require('node:child_process');
const { syncBuiltinESMExports } = require('node:module');
const { EventEmitter } = require('node:events');
const original = cp.exec;
cp.exec = (command, ...args) => {
  const browser = /^(?:start|open|xdg-open)\s+(https?:\/\/\S+)\s*$/.exec(command);
  if (!browser) return original(command, ...args);
  console.log('SMOKE_BROWSER_REQUEST ' + JSON.stringify(browser[1]));
  const callback = args.find(value => typeof value === 'function');
  if (callback) queueMicrotask(() => callback(null, '', ''));
  return new EventEmitter();
};
syncBuiltinESMExports();
"#,
    )?;
    fs::write(script, format!("import './smoke-browser.cjs';\n{source}"))?;
    Ok(())
}

fn record_open_url(url: &str) {
    // Opt-in harness: record SDK browser requests without opening the browser.
    log::info!("SMOKE_SDK_OPEN_URL {url}");
}

fn entry(id: &str, repo: Option<&str>) -> CatalogEntry {
    CatalogEntry {
        id: id.into(),
        name: id.into(),
        author: String::new(),
        source: if repo.is_some() {
            Source::OpenAction
        } else {
            Source::ElgatoArchive
        },
        repository: repo.map(|repo| format!("https://github.com/{repo}")),
        download_url: repo
            .is_none()
            .then(|| format!("https://plugins.amankhanna.me/rezipped/{id}.zip")),
        icon_url: String::new(),
        version: None,
    }
}

async fn cached_archive(
    client: &reqwest::Client,
    cache: &Path,
    entry: &CatalogEntry,
) -> Result<(Vec<u8>, String)> {
    let archive = cache.join(format!("{}.zip", entry.id));
    let source = cache.join(format!("{}.url", entry.id));
    if archive.is_file() && source.is_file() {
        return Ok((fs::read(archive)?, fs::read_to_string(source)?));
    }
    let assets = resolve_download(client, entry).await?;
    let asset = assets
        .iter()
        .find(|a| a.name.starts_with(&entry.id))
        .or_else(|| assets.first())
        .context("release has no plugin archive")?;
    println!("Downloading {} from {}", entry.id, asset.url);
    let response = client.get(&asset.url).send().await?.error_for_status()?;
    anyhow::ensure!(
        response.content_length().unwrap_or(0) <= 300 * 1024 * 1024,
        "archive too large"
    );
    let bytes = response.bytes().await?;
    anyhow::ensure!(bytes.len() <= 300 * 1024 * 1024, "archive too large");
    fs::write(&archive, &bytes)?;
    fs::write(&source, &asset.url)?;
    Ok((bytes.to_vec(), asset.url.clone()))
}

fn event_name(event: &PluginEvent) -> &'static str {
    match event {
        PluginEvent::SetImage { .. } => "SetImage (host sink; may be manifest default)",
        PluginEvent::SetTitle { .. } => "SetTitle",
        PluginEvent::SetState { .. } => "SetState",
        PluginEvent::SetSettings { .. } => "SetSettings",
        PluginEvent::ShowOk { .. } => "ShowOk",
        PluginEvent::ShowAlert { .. } => "ShowAlert",
        PluginEvent::SwitchProfile { .. } => "SwitchProfile",
        PluginEvent::Status { .. } => "Status",
        PluginEvent::TitleStyle { .. } => "TitleStyle (manifest)",
    }
}

fn observe(
    plugin: &LoadedPlugin,
    plugin_root: &Path,
    logs: &Path,
    action: &str,
    press: bool,
) -> Result<Vec<String>> {
    let (events_tx, events_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel();
    let mut guard = HostGuard {
        tx: spawn_host(HostDeps {
            plugins_dir: plugin_root.to_path_buf(),
            settings_file: plugin_root.join("global-settings.json"),
            launcher: Box::new(RecordingLauncher {
                inner: Supervisor::new(logs),
                stopped: stop_tx,
            }),
            events: Box::new(move |event| {
                let _ = events_tx.send(event);
            }),
            decode_image: gerzys_deck::plugins::image::decode_plugin_image,
            open_url: record_open_url,
            list_processes: Vec::new,
        }),
        stopped: Some(stop_rx),
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    guard.tx.send(HostCmd::DeviceConnected)?;
    let key = KeyCtx {
        plugin: plugin.id.clone(),
        ctx: ActionCtx {
            action: action.to_owned(),
            context: format!("smoke:{}:1", plugin.id),
            device: DEVICE_ID.into(),
            key: 1,
            settings: json!({}),
            state: 0,
        },
    };
    guard.tx.send(HostCmd::WillAppear(key.clone()))?;
    let mut events = BTreeSet::new();
    let mut press_at = None;
    let mut pressed = false;
    let mut released = false;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        if press && !pressed && press_at.is_some_and(|at| Instant::now() >= at) {
            guard.tx.send(HostCmd::KeyDown(key.clone()))?;
            pressed = true;
            log::info!("SMOKE_KEY_DOWN {} {action}", plugin.id);
        }
        if pressed
            && !released
            && press_at.is_some_and(|at| Instant::now() >= at + Duration::from_millis(100))
        {
            guard.tx.send(HostCmd::KeyUp(key.clone()))?;
            released = true;
            log::info!("SMOKE_KEY_UP {} {action}", plugin.id);
        }
        match events_rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
            Ok(event) => {
                if press
                    && press_at.is_none()
                    && matches!(
                        event,
                        PluginEvent::Status {
                            status: PluginStatus::Running,
                            ..
                        }
                    )
                {
                    // Let the real SDK consume willAppear/global settings first.
                    press_at = Some(Instant::now() + Duration::from_secs(2));
                }
                events.insert(event_name(&event).to_owned());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                anyhow::bail!("host event channel disconnected")
            }
        }
    }
    guard.shutdown()?;
    anyhow::ensure!(!press || released, "plugin never registered for key press");
    Ok(events.into_iter().collect())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let only = args
        .windows(2)
        .find(|a| a[0] == "--only")
        .map(|a| a[1].clone());
    let wanted = args
        .windows(2)
        .find(|a| a[0] == "--action")
        .map(|a| a[1].as_str());
    let press = args.iter().any(|a| a == "--press");
    let capture_browser = args.iter().any(|a| a == "--capture-node-browser");
    anyhow::ensure!(
        !capture_browser || (press && only.is_some()),
        "browser capture requires --only and --press"
    );
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.superpowers/plugin-smoke")
        });
    fs::create_dir_all(root.join("cache"))?;
    let run = root.join(format!(
        "run-{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        std::process::id()
    ));
    fs::create_dir(&run)?;
    let diagnostics = Arc::new(Mutex::new(Vec::new()));
    log::set_boxed_logger(Box::new(SmokeLogger {
        diagnostics: diagnostics.clone(),
        file: Mutex::new(File::create(run.join("host.log"))?),
        started: Instant::now(),
    }))
    .map_err(|e| anyhow::anyhow!("install smoke logger: {e}"))?;
    log::set_max_level(log::LevelFilter::Debug);
    let client = reqwest::Client::builder()
        .user_agent("GerzysDeck")
        .timeout(Duration::from_secs(120))
        .build()?;
    let targets = [
        entry("me.amankhanna.oasystem", Some("OpenActionPlugins/system")),
        entry("me.amankhanna.oadiscord", Some("OpenActionPlugins/discord")),
        entry("com.elgato.cpu", None),
        entry(
            "com.ntanis.essentials-for-spotify",
            Some("ntanis-dev/essentials-for-spotify"),
        ),
    ];
    anyhow::ensure!(
        only.as_deref()
            .is_none_or(|id| targets.iter().any(|entry| entry.id == id)),
        "unknown --only plugin id"
    );
    let mut results = Vec::new();
    for target in targets
        .into_iter()
        .filter(|entry| only.as_deref().is_none_or(|id| id == entry.id))
    {
        println!("Observing {} (30 seconds)", target.id);
        let logs = run.join(&target.id).join("logs");
        let plugins = run.join(&target.id).join("plugins");
        fs::create_dir_all(&plugins)?;
        let mut row = SmokeResult {
            id: target.id.clone(),
            version: None,
            source_url: None,
            action: None,
            observation: Observation::default(),
            host_events: Vec::new(),
            log_path: logs.join(format!("{}.log", target.id)),
            combined_output_bytes: 0,
            error: None,
            pressed: false,
            browser_requests: Vec::new(),
        };
        let result: Result<()> = async {
            let (bytes, url) = cached_archive(&client, &root.join("cache"), &target).await?;
            row.source_url = Some(url);
            let plugin = install_from_zip(&bytes, &plugins, Some(&target.id))?;
            row.version = plugin.manifest.version.clone();
            let actions: Vec<_> = plugin
                .manifest
                .keypad_actions()
                .map(|a| a.uuid.as_str())
                .collect();
            let action =
                select_action(&actions, wanted).context("requested keypad action missing")?;
            row.action = Some(action.to_owned());
            if capture_browser {
                capture_node_browser(&plugin)?;
            }
            row.host_events = observe(&plugin, &plugins, &logs, action, press)?;
            row.pressed = press;
            Ok(())
        }
        .await;
        if let Err(error) = result {
            row.error = Some(format!("{error:#}"));
        }
        row.observation = summarize(
            &target.id,
            &diagnostics
                .lock()
                .map_err(|_| anyhow::anyhow!("diagnostic mutex poisoned"))?,
        );
        row.combined_output_bytes = fs::metadata(&row.log_path).map(|m| m.len()).unwrap_or(0);
        row.browser_requests =
            browser_requests(&fs::read_to_string(&row.log_path).unwrap_or_default());
        println!("{}", serde_json::to_string(&row)?);
        results.push(row);
        fs::write(
            run.join("results.json"),
            serde_json::to_vec_pretty(&results)?,
        )?;
    }
    fs::write(
        run.join("diagnostics.json"),
        serde_json::to_vec_pretty(
            &*diagnostics
                .lock()
                .map_err(|_| anyhow::anyhow!("diagnostic mutex poisoned"))?,
        )?,
    )?;
    println!("Artifacts: {}", run.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inbound_diagnostics_distinguish_real_updates_and_unknown_events() {
        let diagnostics = vec![
            json!({"plugin":"test","kind":"launched","pid":42}),
            json!({"plugin":"test","kind":"registered"}),
            json!({"plugin":"test","kind":"inbound","event":"setImage","classification":"known"}),
            json!({"plugin":"test","kind":"inbound","event":"customEvent","classification":"unknown"}),
            json!({"plugin":"other","kind":"inbound","event":"setTitle","classification":"known"}),
        ];
        let row = summarize("test", &diagnostics);
        assert!(row.launched && row.registered && row.set_image);
        assert!(!row.set_title);
        assert_eq!(row.unknown, vec!["customEvent"]);
    }

    #[test]
    fn host_default_images_and_invalid_messages_are_not_plugin_updates() {
        let row = summarize(
            "test",
            &[
                json!({"plugin":"test","kind":"host","event":"setImage"}),
                json!({"plugin":"test","kind":"inbound","event":"setTitle","classification":"invalid"}),
            ],
        );
        assert!(!row.launched && !row.registered && !row.set_image && !row.set_title);
    }

    #[test]
    fn explicit_action_and_browser_capture_are_opt_in() {
        let actions = vec!["setup", "play-pause"];
        assert_eq!(select_action(&actions, None), Some("setup"));
        assert_eq!(
            select_action(&actions, Some("play-pause")),
            Some("play-pause")
        );
        assert_eq!(select_action(&actions, Some("missing")), None);
        assert_eq!(
            browser_requests(
                "normal output\nSMOKE_BROWSER_REQUEST \"http://127.0.0.1:6545/?lang=cs\"\n"
            ),
            vec!["http://127.0.0.1:6545/?lang=cs"]
        );
        assert!(browser_requests("SMOKE_BROWSER_REQUEST not-json\n").is_empty());
    }
}

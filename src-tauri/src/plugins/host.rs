//! Plugin host actor: loads installed plugins, launches them, owns the WebSocket server and
//! routes SDK messages between the engine, plugins and property inspectors.

use super::link::{HostCmd, KeyCtx, PiInfo, PluginEvent, PluginStatus};
use super::manifest::Manifest;
use super::protocol::{self, coordinates, info_json, Inbound};
use super::scheme;
use super::server::{self, new_token, Registrant, ServerEvent, ServerOptions, Tokens};
use image::RgbaImage;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// Device id reported to plugins (same as the engine's).
pub const DEVICE_ID: &str = "D6-main";
const QUEUE_CAP: usize = 500;

#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    /// folder name, e.g. `com.elgato.cpu.sdPlugin`
    pub id: String,
    pub dir: PathBuf,
    pub manifest: Manifest,
}

impl LoadedPlugin {
    pub fn supports_windows(&self) -> bool {
        self.manifest.os.is_empty() || self.manifest.os.iter().any(|o| o.platform.eq_ignore_ascii_case("windows"))
    }
}

/// Starts plugin programs (exe/node via the process supervisor, html via hidden webviews).
pub trait Launcher: Send {
    fn start(&mut self, plugin: &LoadedPlugin, port: u16, uuid: &str, info: &str);
    fn stop_all(&mut self);
}

pub struct HostDeps {
    pub plugins_dir: PathBuf,
    pub settings_file: PathBuf,
    pub events: Box<dyn Fn(PluginEvent) + Send>,
    pub launcher: Box<dyn Launcher>,
    pub decode_image: fn(&str, Option<&Path>) -> Option<RgbaImage>,
    pub open_url: fn(&str),
    pub list_processes: fn() -> Vec<String>,
}

/// Per-plugin global settings, persisted as one JSON object keyed by plugin id.
pub struct GlobalSettings {
    file: PathBuf,
    map: serde_json::Map<String, Value>,
}

impl GlobalSettings {
    pub fn load(file: &Path) -> Self {
        let map = std::fs::read_to_string(file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        Self { file: file.to_path_buf(), map }
    }
    pub fn get(&self, plugin: &str) -> Value {
        self.map.get(plugin).cloned().unwrap_or_else(|| json!({}))
    }
    pub fn set(&mut self, plugin: &str, value: Value) {
        self.map.insert(plugin.to_string(), value);
        let tmp = self.file.with_extension("tmp");
        let ok = std::fs::write(&tmp, serde_json::to_vec_pretty(&self.map).unwrap_or_default()).and_then(|_| std::fs::rename(&tmp, &self.file));
        if let Err(e) = ok {
            log::error!("saving plugin global settings failed: {e}");
        }
    }
}

/// Every plugin folder with a readable manifest.
pub fn load_installed(plugins_dir: &Path) -> Vec<LoadedPlugin> {
    let Ok(entries) = std::fs::read_dir(plugins_dir) else { return vec![] };
    let mut out: Vec<LoadedPlugin> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let dir = e.path();
            match Manifest::load(&dir) {
                Ok(manifest) => Some(LoadedPlugin { id: e.file_name().to_string_lossy().into_owned(), dir, manifest }),
                Err(err) => {
                    log::warn!("skipping plugin {}: {err}", dir.display());
                    None
                }
            }
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

struct PluginRt {
    loaded: LoadedPlugin,
    tx: Option<UnboundedSender<String>>,
    queue: Vec<String>,
    ever_connected: bool,
}

struct PiRt {
    uuid: String,
    plugin: String,
    tx: Option<UnboundedSender<String>>,
    queue: Vec<String>,
}

struct Host {
    deps: HostDeps,
    port: u16,
    tokens: Arc<Tokens>,
    plugins: HashMap<String, PluginRt>,
    instances: HashMap<String, KeyCtx>,
    pis: HashMap<String, PiRt>,
    global: GlobalSettings,
    device_connected: bool,
    running_apps: HashSet<String>,
}

fn send_or_queue(tx: &Option<UnboundedSender<String>>, queue: &mut Vec<String>, msg: String) {
    match tx {
        Some(t) if t.send(msg.clone()).is_ok() => {}
        _ => {
            if queue.len() < QUEUE_CAP {
                queue.push(msg);
            }
        }
    }
}

impl Host {
    fn emit(&self, e: PluginEvent) {
        (self.deps.events)(e);
    }

    fn to_plugin(&mut self, plugin: &str, v: Value) {
        if let Some(p) = self.plugins.get_mut(plugin) {
            send_or_queue(&p.tx, &mut p.queue, v.to_string());
        }
    }

    fn to_pi(&mut self, context: &str, v: Value) {
        if let Some(pi) = self.pis.get_mut(context) {
            send_or_queue(&pi.tx, &mut pi.queue, v.to_string());
        }
    }

    fn launch_all(&mut self) {
        for loaded in load_installed(&self.deps.plugins_dir) {
            if !loaded.supports_windows() {
                log::warn!("plugin {} does not support Windows", loaded.id);
                self.emit(PluginEvent::Status { plugin: loaded.id.clone(), status: PluginStatus::Unsupported });
                continue;
            }
            let uuid = new_token();
            self.tokens.insert(uuid.clone(), Registrant::Plugin(loaded.id.clone()));
            let plugin_uuid = loaded.manifest.uuid.clone().unwrap_or_else(|| loaded.id.trim_end_matches(".sdPlugin").to_string());
            let info = info_json(DEVICE_ID, &plugin_uuid, loaded.manifest.version.as_deref().unwrap_or("1.0")).to_string();
            self.deps.launcher.start(&loaded, self.port, &uuid, &info);
            self.emit(PluginEvent::Status { plugin: loaded.id.clone(), status: PluginStatus::Starting });
            self.plugins.insert(loaded.id.clone(), PluginRt { loaded, tx: None, queue: vec![], ever_connected: false });
        }
    }

    /// Default image of an action state from the manifest.
    fn default_image(&self, kc: &KeyCtx) -> Option<RgbaImage> {
        let p = self.plugins.get(&kc.plugin)?;
        let action = p.loaded.manifest.actions.iter().find(|a| a.uuid == kc.ctx.action)?;
        let state = action.states.get(kc.ctx.state as usize).or_else(|| action.states.first())?;
        let base = if state.image.is_empty() { &action.icon } else { &state.image };
        if base.is_empty() {
            return None;
        }
        let path = super::manifest::resolve_image(&p.loaded.dir, base)?;
        let rel = path.strip_prefix(p.loaded.dir.canonicalize().ok()?).ok()?.to_string_lossy().replace('\\', "/");
        (self.deps.decode_image)(&rel, Some(&p.loaded.dir))
    }

    fn owns(&self, plugin: &str, context: &str) -> bool {
        self.instances.get(context).is_some_and(|k| k.plugin == plugin)
    }

    fn on_cmd(&mut self, cmd: HostCmd) -> bool {
        match cmd {
            HostCmd::DeviceConnected => {
                self.device_connected = true;
                let ids: Vec<String> = self.plugins.keys().cloned().collect();
                for id in ids {
                    self.to_plugin(&id, protocol::device_did_connect(DEVICE_ID));
                }
            }
            HostCmd::DeviceDisconnected => {
                self.device_connected = false;
                let ids: Vec<String> = self.plugins.keys().cloned().collect();
                for id in ids {
                    self.to_plugin(&id, protocol::device_did_disconnect(DEVICE_ID));
                }
            }
            HostCmd::WillAppear(kc) => {
                if !self.plugins.contains_key(&kc.plugin) {
                    self.emit(PluginEvent::Status { plugin: kc.plugin.clone(), status: PluginStatus::NotInstalled });
                    return true;
                }
                if let Some(img) = self.default_image(&kc) {
                    self.emit(PluginEvent::SetImage { context: kc.ctx.context.clone(), image: Some(Arc::new(img)), state: None });
                }
                self.to_plugin(&kc.plugin, protocol::appear_event("willAppear", &kc.ctx));
                self.instances.insert(kc.ctx.context.clone(), kc);
            }
            HostCmd::WillDisappear(kc) => {
                self.instances.remove(&kc.ctx.context);
                self.to_plugin(&kc.plugin, protocol::appear_event("willDisappear", &kc.ctx));
            }
            HostCmd::KeyDown(kc) => self.to_plugin(&kc.plugin, protocol::key_event("keyDown", &kc.ctx)),
            HostCmd::KeyUp(kc) => self.to_plugin(&kc.plugin, protocol::key_event("keyUp", &kc.ctx)),
            HostCmd::TitleParams { key, title } => {
                let msg = protocol::title_parameters_did_change(&key.ctx, &title, !title.is_empty(), 9, "#ffffff", "bottom");
                self.to_plugin(&key.plugin, msg);
            }
            HostCmd::PiOpen { key, reply } => {
                let info = self.open_pi(key);
                let _ = reply.send(info);
            }
            HostCmd::PiClose { context } => self.close_pi(&context),
            HostCmd::Shutdown => {
                self.deps.launcher.stop_all();
                return false;
            }
        }
        true
    }

    fn open_pi(&mut self, key: KeyCtx) -> Option<PiInfo> {
        let p = self.plugins.get(&key.plugin)?;
        let action = p.loaded.manifest.actions.iter().find(|a| a.uuid == key.ctx.action)?;
        let pi_path = action.property_inspector_path.clone().or_else(|| p.loaded.manifest.property_inspector_path.clone())?;
        let plugin_uuid = p.loaded.manifest.uuid.clone().unwrap_or_else(|| key.plugin.trim_end_matches(".sdPlugin").to_string());
        let info = info_json(DEVICE_ID, &plugin_uuid, p.loaded.manifest.version.as_deref().unwrap_or("1.0"));
        let context = key.ctx.context.clone();
        self.close_pi(&context);
        let uuid = new_token();
        self.tokens.insert(uuid.clone(), Registrant::Pi { plugin: key.plugin.clone(), context: context.clone() });
        let action_info = json!({
            "action": key.ctx.action, "context": context, "device": DEVICE_ID,
            "payload": { "settings": key.ctx.settings, "coordinates": coordinates(key.ctx.key), "state": key.ctx.state }
        });
        let params = json!({ "port": self.port, "uuid": uuid, "registerEvent": "registerPropertyInspector", "info": info, "actionInfo": action_info });
        let url = scheme::page_url(&key.plugin, &pi_path, &params);
        self.pis.insert(context.clone(), PiRt { uuid: uuid.clone(), plugin: key.plugin.clone(), tx: None, queue: vec![] });
        self.to_plugin(&key.plugin, protocol::pi_event("propertyInspectorDidAppear", &key.ctx));
        Some(PiInfo { url, port: self.port, uuid, info, action_info, context })
    }

    fn close_pi(&mut self, context: &str) {
        if let Some(pi) = self.pis.remove(context) {
            self.tokens.remove(&pi.uuid);
            if let Some(kc) = self.instances.get(context).cloned() {
                self.to_plugin(&pi.plugin, protocol::pi_event("propertyInspectorDidDisappear", &kc.ctx));
            }
        }
    }

    fn update_settings(&mut self, context: &str, settings: Value) -> Option<KeyCtx> {
        let kc = self.instances.get_mut(context)?;
        kc.ctx.settings = settings.clone();
        let kc = kc.clone();
        self.emit(PluginEvent::SetSettings { context: context.to_string(), settings });
        Some(kc)
    }

    fn on_server(&mut self, ev: ServerEvent) {
        match ev {
            ServerEvent::Connected { who: Registrant::Plugin(id), tx, .. } => {
                let resend: Vec<Value> = if self.plugins.get(&id).is_some_and(|p| p.ever_connected) {
                    let mut v = vec![];
                    if self.device_connected {
                        v.push(protocol::device_did_connect(DEVICE_ID));
                    }
                    v.extend(self.instances.values().filter(|k| k.plugin == id).map(|k| protocol::appear_event("willAppear", &k.ctx)));
                    v
                } else {
                    vec![]
                };
                let apps = self.running_monitored(&id);
                if let Some(p) = self.plugins.get_mut(&id) {
                    if !p.ever_connected && self.device_connected && !p.queue.iter().any(|m| m.contains("\"deviceDidConnect\"")) {
                        p.queue.insert(0, protocol::device_did_connect(DEVICE_ID).to_string());
                    }
                    p.ever_connected = true;
                    for m in p.queue.drain(..).chain(resend.into_iter().map(|v| v.to_string())).chain(apps) {
                        let _ = tx.send(m);
                    }
                    p.tx = Some(tx);
                }
                self.emit(PluginEvent::Status { plugin: id, status: PluginStatus::Running });
            }
            ServerEvent::Connected { who: Registrant::Pi { context, .. }, tx, .. } => {
                if let Some(pi) = self.pis.get_mut(&context) {
                    for m in pi.queue.drain(..) {
                        let _ = tx.send(m);
                    }
                    pi.tx = Some(tx);
                }
            }
            ServerEvent::Disconnected { who: Registrant::Plugin(id), .. } => {
                if let Some(p) = self.plugins.get_mut(&id) {
                    p.tx = None;
                }
                self.emit(PluginEvent::Status { plugin: id, status: PluginStatus::Starting });
            }
            ServerEvent::Disconnected { who: Registrant::Pi { context, .. }, uuid } => {
                if let Some(pi) = self.pis.get_mut(&context) {
                    if pi.uuid == uuid {
                        pi.tx = None;
                    }
                }
            }
            ServerEvent::Message { who: Registrant::Plugin(id), msg, .. } => self.from_plugin(&id, msg),
            ServerEvent::Message { who: Registrant::Pi { plugin, context }, msg, .. } => self.from_pi(&plugin, &context, msg),
        }
    }

    fn from_plugin(&mut self, id: &str, msg: Inbound) {
        match msg {
            Inbound::SetImage { context, image, state } if self.owns(id, &context) => {
                let dir = self.plugins.get(id).map(|p| p.loaded.dir.clone());
                let decoded = match &image {
                    Some(src) => (self.deps.decode_image)(src, dir.as_deref()),
                    None => self.instances.get(&context).cloned().and_then(|k| self.default_image(&k)),
                };
                self.emit(PluginEvent::SetImage { context, image: decoded.map(Arc::new), state });
            }
            Inbound::SetTitle { context, title, state } if self.owns(id, &context) => self.emit(PluginEvent::SetTitle { context, title, state }),
            Inbound::SetState { context, state } if self.owns(id, &context) => {
                if let Some(k) = self.instances.get_mut(&context) {
                    k.ctx.state = state;
                }
                self.emit(PluginEvent::SetState { context, state });
            }
            Inbound::SetSettings { context, payload } if self.owns(id, &context) => {
                if let Some(kc) = self.update_settings(&context, payload) {
                    self.to_pi(&context, protocol::did_receive_settings(&kc.ctx));
                }
            }
            Inbound::GetSettings { context } if self.owns(id, &context) => {
                if let Some(kc) = self.instances.get(&context).cloned() {
                    self.to_plugin(id, protocol::did_receive_settings(&kc.ctx));
                }
            }
            Inbound::SetGlobalSettings { payload, .. } => {
                self.global.set(id, payload.clone());
                let contexts: Vec<String> = self.pis.iter().filter(|(_, p)| p.plugin == id).map(|(c, _)| c.clone()).collect();
                for c in contexts {
                    self.to_pi(&c, protocol::did_receive_global_settings(&payload));
                }
            }
            Inbound::GetGlobalSettings { .. } => {
                let g = self.global.get(id);
                self.to_plugin(id, protocol::did_receive_global_settings(&g));
            }
            Inbound::ShowOk { context } if self.owns(id, &context) => self.emit(PluginEvent::ShowOk { context }),
            Inbound::ShowAlert { context } if self.owns(id, &context) => self.emit(PluginEvent::ShowAlert { context }),
            Inbound::OpenUrl { url } => (self.deps.open_url)(&url),
            Inbound::LogMessage { message } => log::info!("[{id}] {message}"),
            Inbound::SwitchToProfile { profile: Some(profile) } => self.emit(PluginEvent::SwitchProfile { profile }),
            Inbound::SendToPropertyInspector { context, payload } if self.owns(id, &context) => {
                let action = self.instances.get(&context).map(|k| k.ctx.action.clone()).unwrap_or_default();
                self.to_pi(&context, protocol::relay("sendToPropertyInspector", &action, &context, &payload));
            }
            other => log::debug!("[{id}] ignored {other:?}"),
        }
    }

    fn from_pi(&mut self, plugin: &str, context: &str, msg: Inbound) {
        match msg {
            Inbound::SetSettings { payload, .. } => {
                if let Some(kc) = self.update_settings(context, payload) {
                    self.to_plugin(plugin, protocol::did_receive_settings(&kc.ctx));
                }
            }
            Inbound::GetSettings { .. } => {
                if let Some(kc) = self.instances.get(context).cloned() {
                    self.to_pi(context, protocol::did_receive_settings(&kc.ctx));
                }
            }
            Inbound::SetGlobalSettings { payload, .. } => {
                self.global.set(plugin, payload.clone());
                self.to_plugin(plugin, protocol::did_receive_global_settings(&payload));
            }
            Inbound::GetGlobalSettings { .. } => {
                let g = self.global.get(plugin);
                self.to_pi(context, protocol::did_receive_global_settings(&g));
            }
            Inbound::SendToPlugin { payload, .. } => {
                let action = self.instances.get(context).map(|k| k.ctx.action.clone()).unwrap_or_default();
                self.to_plugin(plugin, protocol::relay("sendToPlugin", &action, context, &payload));
            }
            Inbound::OpenUrl { url } => (self.deps.open_url)(&url),
            Inbound::LogMessage { message } => log::info!("[{plugin} PI] {message}"),
            other => log::debug!("[{plugin} PI] ignored {other:?}"),
        }
    }

    /// applicationDidLaunch messages for monitored apps that are already running.
    fn running_monitored(&self, id: &str) -> Vec<String> {
        let Some(p) = self.plugins.get(id) else { return vec![] };
        p.loaded
            .manifest
            .applications_to_monitor_windows
            .iter()
            .filter(|app| self.running_apps.contains(&app.to_lowercase()))
            .map(|app| protocol::application_event("applicationDidLaunch", app).to_string())
            .collect()
    }

    fn poll_apps(&mut self) {
        let now: HashSet<String> = (self.deps.list_processes)().into_iter().map(|n| n.to_lowercase()).collect();
        let mut msgs = vec![];
        for (id, p) in &self.plugins {
            for app in &p.loaded.manifest.applications_to_monitor_windows {
                let a = app.to_lowercase();
                match (self.running_apps.contains(&a), now.contains(&a)) {
                    (false, true) => msgs.push((id.clone(), protocol::application_event("applicationDidLaunch", app))),
                    (true, false) => msgs.push((id.clone(), protocol::application_event("applicationDidTerminate", app))),
                    _ => {}
                }
            }
        }
        self.running_apps = now;
        for (id, m) in msgs {
            self.to_plugin(&id, m);
        }
    }
}

/// Starts the host on its own thread with a tokio runtime.
pub fn spawn_host(deps: HostDeps) -> Sender<HostCmd> {
    let (tx, rx) = std::sync::mpsc::channel::<HostCmd>();
    std::thread::Builder::new()
        .name("plugin-host".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    log::error!("plugin host runtime failed: {e}");
                    return;
                }
            };
            rt.block_on(run(deps, rx));
        })
        .expect("spawn plugin host");
    tx
}

async fn run(deps: HostDeps, rx: std::sync::mpsc::Receiver<HostCmd>) {
    let (cmd_tx, mut cmd_rx): (UnboundedSender<HostCmd>, UnboundedReceiver<HostCmd>) = unbounded_channel();
    std::thread::spawn(move || {
        for c in rx {
            if cmd_tx.send(c).is_err() {
                break;
            }
        }
    });
    let tokens = Arc::new(Tokens::default());
    let (ev_tx, mut ev_rx) = unbounded_channel();
    let server = match server::start(tokens.clone(), ev_tx, ServerOptions::default()).await {
        Ok(s) => s,
        Err(e) => {
            log::error!("plugin websocket server failed: {e}");
            return;
        }
    };
    log::info!("plugin host listening on 127.0.0.1:{}", server.port);
    let global = GlobalSettings::load(&deps.settings_file);
    let mut host = Host {
        deps,
        port: server.port,
        tokens,
        plugins: HashMap::new(),
        instances: HashMap::new(),
        pis: HashMap::new(),
        global,
        device_connected: false,
        running_apps: HashSet::new(),
    };
    host.running_apps = (host.deps.list_processes)().into_iter().map(|n| n.to_lowercase()).collect();
    host.launch_all();
    let mut apps_tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(c) => if !host.on_cmd(c) { break },
                None => { host.deps.launcher.stop_all(); break }
            },
            Some(ev) = ev_rx.recv() => host.on_server(ev),
            _ = apps_tick.tick() => host.poll_apps(),
        }
    }
}
